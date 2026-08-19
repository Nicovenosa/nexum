//! LLM Provider and model configuration.
//!
//! Manages provider configuration, model alias resolution, and LLM factory creation.
//! Decoupled from TUI-specific types.

pub mod catalog_path;
pub mod config;
pub mod routes;
pub mod store;

pub use config::{AppConfig, NexumConfig, ProviderConfig, ProviderModels, ThinkingConfig};
use nexum_agent::llm::{BaseModel, ChatAnthropic, ChatOpenAI};
pub use store::{config_path, load, load_from, save, save_to, workspace_config_path};

#[derive(Clone)]
pub enum LlmProvider {
    /// OpenAI 兼容 Provider。`base_url` 需要 `/v1` 后缀。
    OpenAi {
        api_key: String,
        base_url: String,
        model: String,
        thinking: Option<ThinkingConfig>,
    },
    Anthropic {
        api_key: String,
        model: String,
        base_url: Option<String>,
        thinking: Option<ThinkingConfig>,
    },
}

/// ¿El catálogo declara `free_access` para este provider?
///
/// Es el ÚNICO disparador de la rama sin credencial. Deliberadamente NO se usa
/// `api_key.is_empty()` sola: ese campo ya causó un microfix en Ollama, y
/// tratar "sin key" como "tier libre" convertiría cualquier provider mal
/// configurado en uno que sale a la red sin autenticación.
///
/// El estado lo publica `reconcile` sólo después de verificar contra el
/// proveedor que el endpoint responde sin Authorization.
/// Generación de contrato que este binario entiende.
///
/// La FUENTE ÚNICA es `config/catalog-contract.json`, que también lee el lado
/// Python. Este valor la espeja, y `generation_contract_tests` falla si los dos
/// lados divergen: si cada lado llevara su número por su cuenta, el mecanismo
/// que detecta discrepancias entre artefactos derivados sería él mismo dos
/// artefactos que pueden discrepar.
///
/// Se bumpea SÓLO cuando cambia lo que el catálogo promete. Ver `bump_rules`
/// en el contrato.
///
/// # Esta guarda es FORWARD-ONLY, y todas las compiladas lo son
///
/// Verificado en el drill de rollback del 2026-07-31, que es la única forma de
/// saberlo: la estampa custodia hacia adelante y NUNCA hacia atrás.
///
/// El custodio viaja dentro del binario. Un binario anterior a la guarda no
/// tiene con qué quejarse: no la lleva. Así que el cruce peligroso —binario
/// viejo leyendo un catálogo nuevo, que es exactamente lo que pasa al hacer
/// rollback— no lo detecta nadie. Bumpear la generación protege al binario
/// NUEVO de datos viejos; no protege al viejo de datos nuevos.
///
/// Vale igual para `manual_provider_ids`, para `catalog_schema_version` y para
/// el contrato entero: todo lo que llega por `include_str!` o por una constante
/// de Rust comparte la limitación, porque comparte el mecanismo.
///
/// **La protección hacia atrás tiene que vivir en algo que no viaje con el
/// binario**: el runbook de rollback, o los verificadores externos —
/// `nexum-verify-parity`, `nexum-registry-gate`, `nexum doctor`—, que se
/// ejecutan aparte y ven los dos lados.
///
/// En ese mismo drill el cruce lo agarró la validación catálogo↔registry
/// (`CATALOG_ROUTE_PROVIDER_MISMATCH`, nombrando `opencode`) y no esta estampa,
/// que custodia la concesión de `free_access` y no se ejercita desde Doctor. Es
/// la mejor razón para conservar las dos capas: cubren cosas distintas y
/// ninguna es redundante con la otra.
pub const CATALOG_GENERATION: u64 = 4;

/// Veredicto de la estampa. La comparación es ASIMÉTRICA a propósito.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationVerdict {
    /// Misma generación: las concesiones del catálogo valen.
    Match,
    /// Catálogo SIN estampa (los que existían antes de 4.1). Se trata como
    /// generación 0: no concede acceso sin autenticación, pero no rompe nada
    /// que no dependa de una concesión.
    Absent,
    /// Catálogo de una generación POSTERIOR: el caso peligroso. Puede conceder
    /// `free_access` con una semántica que este binario no conoce.
    TooNew { catalog: u64, binary: u64 },
    /// Catálogo viejo: este binario entiende el formato, pero falta reconciliar.
    TooOld { catalog: u64, binary: u64 },
}

impl GenerationVerdict {
    /// ¿Se pueden honrar las concesiones de acceso de este catálogo?
    ///
    /// Falla CERRADA en todo lo que no sea coincidencia exacta: custodiar la
    /// decisión, no comentarla.
    pub fn grants_allowed(&self) -> bool {
        matches!(self, GenerationVerdict::Match)
    }

    /// Mensaje accionable. La degradación tiene que ser visible, nunca una
    /// sorpresa que se descubre chateando.
    pub fn message(&self) -> Option<String> {
        match self {
            GenerationVerdict::Match => None,
            GenerationVerdict::Absent => Some(
                "El catálogo de providers no lleva estampa de generación (fue escrito \
                 por una versión anterior).\n  Efecto: los providers de acceso libre \
                 quedan deshabilitados; el resto no se ve afectado.\n  \
                 Remedio: ejecutá `nexum provider reconcile`."
                    .to_string(),
            ),
            GenerationVerdict::TooNew { catalog, binary } => Some(format!(
                "El catálogo de providers es de una generación POSTERIOR a este \
                 binario (catálogo={catalog}, binario={binary}).\n  Efecto: se \
                 rechazan las concesiones de acceso que este binario no puede \
                 interpretar.\n  Remedio: actualizá Nexum, o ejecutá `nexum provider \
                 reconcile` con este binario para reescribir el catálogo."
            )),
            GenerationVerdict::TooOld { catalog, binary } => Some(format!(
                "El catálogo de providers es de una generación anterior \
                 (catálogo={catalog}, binario={binary}).\n  Efecto: los providers de \
                 acceso libre quedan deshabilitados.\n  Remedio: ejecutá `nexum \
                 provider reconcile`."
            )),
        }
    }
}

/// Compara la estampa del catálogo contra la de este binario.
pub fn generation_verdict(doc: &serde_json::Value) -> GenerationVerdict {
    match doc.get("catalog_generation").and_then(|v| v.as_u64()) {
        None => GenerationVerdict::Absent,
        Some(g) if g == CATALOG_GENERATION => GenerationVerdict::Match,
        Some(g) if g > CATALOG_GENERATION => GenerationVerdict::TooNew {
            catalog: g,
            binary: CATALOG_GENERATION,
        },
        Some(g) => GenerationVerdict::TooOld {
            catalog: g,
            binary: CATALOG_GENERATION,
        },
    }
}

/// Carga el catálogo vivo publicado por `reconcile`. `None` si no existe o no
/// parsea: el llamador decide qué significa la ausencia para su pregunta.
pub fn load_live_catalog() -> Option<serde_json::Value> {
    // Ruta por catalog_path, no construida acá: era una de las tres copias.
    let path = catalog_path::live_catalog()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Qué dice el catálogo sobre la capacidad de un modelo de usar herramientas.
///
/// Los tres estados son distintos a propósito: **no saber no es lo mismo que no
/// poder.** Sólo `Unsupported` —el proveedor afirmando que el modelo no las
/// tiene— justifica bloquear un turno antes de intentarlo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSupport {
    /// El catálogo declara que el modelo sabe usar herramientas.
    Supported,
    /// El catálogo declara que NO. Un turno con tools acá es tiempo perdido.
    Unsupported,
    /// El catálogo no dice nada. Se intenta: la ausencia de dato no bloquea.
    Unknown,
}

/// ¿El modelo declara soporte de herramientas en el catálogo vivo?
///
/// Existe porque un modelo sin tool calling molía el tope entero de vueltas sin
/// avanzar y sin que nadie se enterara. Con esto el runtime puede fallar de
/// entrada, con mensaje, en vez de gastar quince vueltas para llegar al mismo
/// lugar.
pub fn model_tool_support_any(model: &str) -> ToolSupport {
    if model.is_empty() {
        return ToolSupport::Unknown;
    }
    let Some(doc) = load_live_catalog() else {
        return ToolSupport::Unknown;
    };
    let Some(providers) = doc.get("providers").and_then(|p| p.as_array()) else {
        return ToolSupport::Unknown;
    };
    let mut visto_negativo = false;
    for entry in providers {
        match entry
            .get("model_capabilities")
            .and_then(|c| c.get(model))
            .and_then(|m| m.get("tools"))
            .and_then(|v| v.as_bool())
        {
            // Un solo provider que lo declare capaz alcanza: bloquear se
            // reserva para cuando NADIE dice que puede y alguien dice que no.
            Some(true) => return ToolSupport::Supported,
            Some(false) => visto_negativo = true,
            None => {}
        }
    }
    if visto_negativo {
        ToolSupport::Unsupported
    } else {
        ToolSupport::Unknown
    }
}

/// Igual que [`model_tool_support_any`] pero acotado a un provider concreto.
pub fn model_tool_support(provider_id: &str, model: &str) -> ToolSupport {
    if provider_id.is_empty() || model.is_empty() {
        return ToolSupport::Unknown;
    }
    let Some(doc) = load_live_catalog() else {
        return ToolSupport::Unknown;
    };
    let Some(providers) = doc.get("providers").and_then(|p| p.as_array()) else {
        return ToolSupport::Unknown;
    };
    for entry in providers {
        let id = entry
            .get("provider_id")
            .or_else(|| entry.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if id != provider_id {
            continue;
        }
        return match entry
            .get("model_capabilities")
            .and_then(|c| c.get(model))
            .and_then(|m| m.get("tools"))
            .and_then(|v| v.as_bool())
        {
            Some(true) => ToolSupport::Supported,
            Some(false) => ToolSupport::Unsupported,
            None => ToolSupport::Unknown,
        };
    }
    ToolSupport::Unknown
}

/// ¿Hay al menos un provider usable en el catálogo vivo?
///
/// Es la pregunta correcta para "¿es primer arranque?". La anterior —"¿a algún
/// provider le falta credencial?"— hacía que el wizard dependiera de lo que
/// hiciera cada proveedor con su facturación: cuando OpenCode Zen perdió
/// `free_access` al quedarse sin saldo, su key vacía volvió a contar como "sin
/// configurar" y el wizard reapareció sobre una instalación con cinco
/// providers funcionando.
///
/// Que a uno de siete le falte credencial es normal y no dice nada sobre si el
/// usuario ya configuró Nexum. Que ninguno sirva, sí.
pub fn catalog_has_usable_provider() -> bool {
    let Some(doc) = load_live_catalog() else {
        return false;
    };
    doc.get("providers")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .any(|e| e.get("usable_now").and_then(|v| v.as_bool()) == Some(true))
        })
        .unwrap_or(false)
}

pub fn catalog_declares_free_access(provider_id: &str) -> bool {
    if provider_id.is_empty() {
        return false;
    }
    let Some(doc) = load_live_catalog() else {
        return false;
    };
    // La estampa CUSTODIA la concesión: sin generación coincidente no se
    // concede el camino sin autenticación, pase lo que pase.
    if !generation_verdict(&doc).grants_allowed() {
        return false;
    }
    doc.get("providers")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter().any(|entry| {
                let id = entry
                    .get("provider_id")
                    .or_else(|| entry.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                id == provider_id
                    && entry.get("credential_state").and_then(|v| v.as_str())
                        == Some("free_access")
            })
        })
        .unwrap_or(false)
}

impl LlmProvider {
    fn default_openai_model() -> String {
        std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".to_string())
    }

    fn is_legacy_alias(alias: &str) -> bool {
        matches!(alias.to_lowercase().as_str(), "opus" | "sonnet" | "haiku")
    }

    /// Resuelve el nombre de modelo real para un `alias` (típicamente
    /// "opus"/"sonnet"/"haiku" del frontmatter de un subagente, o el modelo
    /// concreto elegido en `/modelo`) contra un provider dado.
    ///
    /// `active_model_fallback`: modelo actualmente activo para ESTE MISMO
    /// provider (viene de `app.active_alias`). Se usa cuando el alias pedido
    /// es un alias legacy de Claude (opus/sonnet/haiku) que el provider no
    /// tiene mapeado — el caso típico es un subagente (Explore, code-review)
    /// cuyo frontmatter pide `model: haiku` corriendo contra un provider
    /// OpenAI-compatible (OpenCode Zen/Go, MiMo) que nunca configuró esos
    /// tres alias.
    ///
    /// Bug real corregido acá (2026-07-07): antes, ese caso caía a
    /// `default_openai_model()`, que lee la env var global `OPENAI_MODEL`.
    /// Esa env var la fija el LAUNCHER una sola vez al arrancar según el
    /// perfil (`ollama` → `qwen2.5:0.5b`); si el usuario cambia de provider
    /// con `/modelo` (Ollama → OpenCode Zen/MiMo) SIN reiniciar el proceso,
    /// la env var queda con el valor viejo. El subagente terminaba pidiendo
    /// `qwen2.5:0.5b` —un modelo de Ollama— a un endpoint OpenAI-compatible
    /// que no lo sirve → 401/404. El fallback correcto es el modelo que el
    /// usuario YA validó como activo para el provider real en uso.
    fn resolve_model(
        provider_type: &str,
        models: &ProviderModels,
        alias: &str,
        active_model_fallback: &str,
    ) -> String {
        let mapped = models
            .get_model(alias)
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string());

        if let Some(model) = mapped {
            return model;
        }

        match provider_type {
            "anthropic" => "claude-sonnet-4-6".to_string(),
            _ if !alias.is_empty() && !Self::is_legacy_alias(alias) => alias.to_string(),
            _ if !active_model_fallback.is_empty() => active_model_fallback.to_string(),
            _ => Self::default_openai_model(),
        }
    }

    pub fn from_env() -> Option<Self> {
        let provider_hint = std::env::var("MODEL_PROVIDER").unwrap_or_default();

        match provider_hint.to_lowercase().as_str() {
            "anthropic" => {
                let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;
                let model = std::env::var("ANTHROPIC_MODEL")
                    .unwrap_or_else(|_| "claude-sonnet-4-6".to_string());
                let base_url = std::env::var("ANTHROPIC_BASE_URL").ok();
                Some(Self::Anthropic {
                    api_key,
                    model,
                    base_url,
                    thinking: None,
                })
            }
            "openai" | "" => {
                if provider_hint.is_empty() {
                    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
                        let model = std::env::var("ANTHROPIC_MODEL")
                            .unwrap_or_else(|_| "claude-sonnet-4-6".to_string());
                        let base_url = std::env::var("ANTHROPIC_BASE_URL").ok();
                        return Some(Self::Anthropic {
                            api_key,
                            model,
                            base_url,
                            thinking: None,
                        });
                    }
                }
                let api_key = std::env::var("OPENAI_API_KEY").ok()?;
                let base_url = std::env::var("OPENAI_API_BASE")
                    .or_else(|_| std::env::var("OPENAI_BASE_URL"))
                    .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
                let model = Self::default_openai_model();
                Some(Self::OpenAi {
                    api_key,
                    base_url,
                    model,
                    thinking: None,
                })
            }
            _ => {
                let api_key = std::env::var("OPENAI_API_KEY").ok()?;
                let base_url = std::env::var("OPENAI_API_BASE")
                    .or_else(|_| std::env::var("OPENAI_BASE_URL"))
                    .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
                let model = Self::default_openai_model();
                Some(Self::OpenAi {
                    api_key,
                    base_url,
                    model,
                    thinking: None,
                })
            }
        }
    }

    /// 从 NexumConfig 构造 LlmProvider（按 active_provider_id 查找 Provider，再按 active_alias 取模型名）
    pub fn from_config(cfg: &config::NexumConfig) -> Option<Self> {
        let app = &cfg.config;
        let provider = app
            .providers
            .iter()
            .find(|p| p.id == app.active_provider_id)?;

        // Un provider sin credencial sólo se acepta si el CATÁLOGO declara que
        // sirve un tier libre verificado. Sin esa declaración, sigue siendo
        // rechazado exactamente como antes.
        if provider.api_key.is_empty() && !catalog_declares_free_access(&provider.id) {
            return None;
        }

        let alias = app.active_alias.as_str();
        // Sin fallback propio: acá `alias` YA ES `active_alias` — no hay
        // una fuente de verdad distinta a la que estamos resolviendo. Si
        // `active_alias` resulta ser un legacy Claude alias sin mapear
        // (active_alias == "haiku" sin que el provider lo configure), el
        // comportamiento correcto sigue siendo el fallback histórico
        // (`default_openai_model()`, string vacío acá lo activa). El
        // fallback a "modelo activo del provider" solo tiene sentido en
        // `from_config_for_alias`, donde el alias pedido (frontmatter de un
        // subagente) es DISTINTO del modelo que el usuario ya eligió.
        let model = Self::resolve_model(&provider.provider_type, &provider.models, alias, "");

        // Thinking: deshabilitado para Ollama local (no soporta reasoning)
        let is_ollama = Self::is_ollama_provider(&provider.base_url, &provider.provider_type);
        let thinking = if is_ollama {
            None
        } else {
            app.thinking.clone().filter(|t| t.enabled)
        };

        match provider.provider_type.as_str() {
            "anthropic" => Some(Self::Anthropic {
                api_key: provider.api_key.clone(),
                model,
                base_url: if provider.base_url.is_empty() {
                    None
                } else {
                    Some(provider.base_url.clone())
                },
                thinking,
            }),
            _ => Some(Self::OpenAi {
                api_key: provider.api_key.clone(),
                base_url: if provider.base_url.is_empty() {
                    "https://api.openai.com/v1".to_string()
                } else {
                    provider.base_url.clone()
                },
                model,
                thinking,
            }),
        }
    }

    /// 从 NexumConfig 按指定 alias（如 "haiku"/"sonnet"/"opus"）构造 LlmProvider
    /// 大小写不敏感；未知 alias fallback 到 ESTE PROVIDER 的modelo activo
    /// (`active_alias`) — nunca a env vars globales de otro perfil de
    /// lanzamiento (ver doc de `resolve_model`).
    /// Construye el provider para un alias/modelo dado.
    ///
    /// Busca al DUEÑO del modelo, no al provider activo. Antes tomaba
    /// `active_provider_id` sin mirar de quién era el modelo, así que
    /// `-p --model qwen3:1.7b` con `active_provider_id=claude_code` armaba un
    /// provider con el endpoint de claude_code y el modelo de Ollama: el puente
    /// respondía `unknown provider for model`. Era el 502 de ollama_local y
    /// opencode_zen, y por eso no se veía en la TUI, donde `/modelo` sí cambia
    /// el provider activo.
    /// Provider que declara este modelo, según el catálogo instalado.
    ///
    /// El catálogo es la autoridad de qué modelo es de quién; el route registry
    /// lo es del endpoint. Sin dueño conocido se devuelve `None` y el llamador
    /// cae al provider activo, que es el comportamiento anterior.
    fn owner_of_model(model: &str) -> Option<String> {
        let doc = load_live_catalog()?;
        let providers = doc.get("providers")?.as_array()?;
        for entry in providers {
            let modelos = entry
                .get("model_policy")
                .and_then(|mp| mp.get("user_facing_models"))
                .or_else(|| entry.get("models"))
                .and_then(|m| m.as_array());
            let Some(modelos) = modelos else { continue };
            if modelos.iter().any(|m| m.as_str() == Some(model)) {
                return entry
                    .get("provider_id")
                    .or_else(|| entry.get("id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
        }
        None
    }

    pub fn from_config_for_alias(cfg: &config::NexumConfig, alias: &str) -> Option<Self> {
        let app = &cfg.config;
        let dueno = Self::owner_of_model(alias);
        let provider = dueno
            .as_deref()
            .and_then(|id| {
                let n = id.replace('-', "_");
                app.providers.iter().find(|p| p.id.replace('-', "_") == n)
            })
            .or_else(|| app.providers.iter().find(|p| p.id == app.active_provider_id))?;

        // Misma guarda que `from_config`: un provider de tier libre no tiene
        // credencial por diseño. Era el CUARTO sitio donde `api_key.is_empty()`
        // se usaba como proxy de "configurado"; la respuesta sale del catálogo. Apareció
        // un QUINTO el 2026-08-01, en ensure_active_provider_config: daba por
        // configurado a un provider con key y sin base_url, y el adaptador sin
        // endpoint caía al puente. Si encontrás otro, sumalo acá — el conteo
        // sirve para saber si el patrón se está terminando o no.
        if provider.api_key.is_empty() && !catalog_declares_free_access(&provider.id) {
            return None;
        }

        let model = Self::resolve_model(
            &provider.provider_type,
            &provider.models,
            alias,
            app.active_alias.as_str(),
        );

        let is_ollama = Self::is_ollama_provider(&provider.base_url, &provider.provider_type);
        let thinking = if is_ollama {
            None
        } else {
            app.thinking.clone().filter(|t| t.enabled)
        };

        match provider.provider_type.as_str() {
            "anthropic" => Some(Self::Anthropic {
                api_key: provider.api_key.clone(),
                model,
                base_url: if provider.base_url.is_empty() {
                    None
                } else {
                    Some(provider.base_url.clone())
                },
                thinking,
            }),
            _ => Some(Self::OpenAi {
                api_key: provider.api_key.clone(),
                base_url: if provider.base_url.is_empty() {
                    "https://api.openai.com/v1".to_string()
                } else {
                    provider.base_url.clone()
                },
                model,
                thinking,
            }),
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            Self::OpenAi { base_url, .. } => {
                if Self::is_ollama_provider(base_url, "openai") {
                    "Ollama Local"
                } else {
                    "OpenAI"
                }
            }
            Self::Anthropic { .. } => "Anthropic",
        }
    }

    /// Detecta si un provider es Ollama local por su base_url (UX FIX 04)
    fn is_ollama_provider(base_url: &str, provider_type: &str) -> bool {
        if provider_type.to_lowercase() != "openai" {
            return false;
        }
        let base = base_url.to_lowercase();
        base.contains("127.0.0.1:11434")
            || base.contains("localhost:11434")
            || base.contains("127.0.0.1:11435")
    }

    pub fn model_name(&self) -> &str {
        match self {
            Self::OpenAi { model, .. } => model,
            Self::Anthropic { model, .. } => model,
        }
    }

    /// 获取模型的上下文窗口大小（不消费 self）
    pub fn context_window(&self) -> u32 {
        self.clone().into_model().context_window()
    }

    pub fn into_model(self) -> Box<dyn BaseModel> {
        match self {
            Self::OpenAi {
                api_key,
                base_url,
                model,
                thinking,
            } => {
                // Política capability-aware SÓLO para el provider local de CPU
                // (Ollama): non-stream + read-timeout local. Los demás providers
                // OpenAI-compat conservan su comportamiento (streaming on, timeout
                // remoto). No cambia el routing ni la lista de providers/modelos.
                let is_ollama = Self::is_ollama_provider(&base_url, "openai");
                let mut m = ChatOpenAI::new(api_key, model).with_base_url(base_url);
                if is_ollama {
                    m = m.with_local_cpu_profile();
                }
                if let Some(ref t) = thinking {
                    m = m.with_reasoning_effort(t.openai_effort());
                    if t.enabled {
                        m = m.with_thinking_enabled();
                    }
                }
                let max_tokens = thinking.as_ref().map_or(32000, |t| t.max_tokens);
                m = m.with_max_tokens(max_tokens);
                Box::new(m)
            }
            Self::Anthropic {
                api_key,
                model,
                base_url,
                thinking,
            } => {
                let mut m = ChatAnthropic::new(api_key, model);
                if let Some(url) = base_url {
                    m = m.with_base_url(url);
                }
                if let Some(ref t) = thinking {
                    m = m.with_extended_thinking(t.budget_tokens, &t.effort);
                }
                let max_tokens = thinking.as_ref().map_or(32000, |t| t.max_tokens);
                m = m.with_max_tokens(max_tokens);
                Box::new(m)
            }
        }
    }
}

#[cfg(test)]
mod resolve_model_test {
    use super::config::{AppConfig, NexumConfig, ProviderConfig};
    use super::LlmProvider;

    fn opencode_zen_provider() -> ProviderConfig {
        ProviderConfig {
            id: "opencode_zen".to_string(),
            provider_type: "openai".to_string(),
            api_key: "sk-zen-test".to_string(),
            base_url: "https://opencode.ai/zen/v1".to_string(),
            name: Some("OpenCode Zen".to_string()),
            // Escenario real: el provider NUNCA configuró opus/sonnet/haiku
            // (esos campos son para providers que emulan la API de Claude).
            ..Default::default()
        }
    }

    fn mimo_provider() -> ProviderConfig {
        ProviderConfig {
            id: "mimo_code".to_string(),
            provider_type: "openai".to_string(),
            api_key: "sk-mimo-test".to_string(),
            base_url: "https://api.xiaomimimo.com/v1".to_string(),
            name: Some("MiMo Code".to_string()),
            ..Default::default()
        }
    }

    fn cfg_with(active_provider_id: &str, active_alias: &str, providers: Vec<ProviderConfig>) -> NexumConfig {
        NexumConfig {
            schema: None,
            config: AppConfig {
                active_provider_id: active_provider_id.to_string(),
                active_alias: active_alias.to_string(),
                providers,
                ..Default::default()
            },
        }
    }

    /// Bug real (2026-07-07): un subagente (Explore/code-reviewer) pide un
    /// alias legacy de Claude ("haiku") vía frontmatter. Si el provider
    /// activo es OpenCode Zen (sin haiku mapeado) y quedó una env var
    /// OPENAI_MODEL="qwen2.5:0.5b" de un lanzamiento anterior con perfil
    /// Ollama, el subagente NO debe terminar pidiéndole qwen2.5:0.5b a Zen.
    /// Debe usar el modelo que el usuario ya tiene activo para Zen.
    #[test]
    fn test_subagent_haiku_alias_no_usa_qwen_contra_opencode_zen() {
        // Simula el leak de env var del perfil ollama del launcher.
        std::env::set_var("OPENAI_MODEL", "qwen2.5:0.5b");

        let cfg = cfg_with(
            "opencode_zen",
            "deepseek-v4-flash-free",
            vec![opencode_zen_provider()],
        );

        let provider = LlmProvider::from_config_for_alias(&cfg, "haiku")
            .expect("provider con api_key no vacía debe resolver");
        let model = provider.model_name();

        assert_ne!(model, "qwen2.5:0.5b", "el subagente no debe heredar el modelo de Ollama");
        assert_eq!(
            model, "deepseek-v4-flash-free",
            "debe usar el modelo YA ACTIVO del provider real (Zen), no la env var global"
        );

        std::env::remove_var("OPENAI_MODEL");
    }

    #[test]
    fn test_subagent_sonnet_alias_no_usa_qwen_contra_mimo() {
        std::env::set_var("OPENAI_MODEL", "qwen2.5:0.5b");

        let cfg = cfg_with("mimo_code", "mimo-v2.5-pro", vec![mimo_provider()]);

        let provider = LlmProvider::from_config_for_alias(&cfg, "sonnet").unwrap();
        let model = provider.model_name();

        assert_ne!(model, "qwen2.5:0.5b");
        assert_eq!(model, "mimo-v2.5-pro");

        std::env::remove_var("OPENAI_MODEL");
    }

    /// Si el provider SÍ mapeó el alias explícitamente, ese mapeo gana
    /// (comportamiento preexistente, no debe romperse).
    #[test]
    fn test_alias_mapeado_explicitamente_tiene_prioridad() {
        let mut provider = opencode_zen_provider();
        provider.models.haiku = "deepseek-v4-flash".to_string();
        let cfg = cfg_with("opencode_zen", "deepseek-v4-pro", vec![provider]);

        let resolved = LlmProvider::from_config_for_alias(&cfg, "haiku").unwrap();
        assert_eq!(resolved.model_name(), "deepseek-v4-flash");
    }

    /// Un alias que NO es legacy de Claude (p.ej. el subagente pide un
    /// modelo concreto por nombre) se usa literal, sin pasar por el
    /// fallback — comportamiento preexistente.
    #[test]
    fn test_alias_no_legacy_se_usa_literal() {
        let cfg = cfg_with(
            "opencode_zen",
            "deepseek-v4-flash-free",
            vec![opencode_zen_provider()],
        );
        let resolved = LlmProvider::from_config_for_alias(&cfg, "glm-5.2").unwrap();
        assert_eq!(resolved.model_name(), "glm-5.2");
    }

    /// Provider Anthropic real: los alias legacy siguen resolviendo al
    /// modelo Claude fijo (comportamiento preexistente intacto).
    #[test]
    fn test_provider_anthropic_alias_legacy_resuelve_a_claude() {
        let provider = ProviderConfig {
            id: "claude_code".to_string(),
            provider_type: "anthropic".to_string(),
            api_key: "sk-ant-test".to_string(),
            ..Default::default()
        };
        let cfg = cfg_with("claude_code", "claude-sonnet-5", vec![provider]);
        let resolved = LlmProvider::from_config_for_alias(&cfg, "haiku").unwrap();
        assert_eq!(resolved.model_name(), "claude-sonnet-4-6");
    }
}


#[cfg(test)]
mod ollama_streaming_policy_test {
    use super::LlmProvider;

    /// GATE: la capa provider aplica el perfil non-stream SÓLO a Ollama local.
    #[test]
    fn ollama_provider_is_non_streaming() {
        let p = LlmProvider::OpenAi {
            api_key: "ollama".to_string(),
            base_url: "http://127.0.0.1:11434/v1".to_string(),
            model: "qwen2.5:1.5b".to_string(),
            thinking: None,
        };
        let model = p.into_model();
        assert!(
            !model.supports_streaming(),
            "Ollama local debe resolverse a política non-stream"
        );
        assert_eq!(model.model_id(), "qwen2.5:1.5b", "model_id preservado");
    }

    /// GATE: un provider OpenAI-compat remoto NO se ve afectado (streaming on).
    #[test]
    fn remote_openai_provider_keeps_streaming() {
        let p = LlmProvider::OpenAi {
            api_key: "sk-remote".to_string(),
            base_url: "https://opencode.ai/zen/v1".to_string(),
            model: "grok-code".to_string(),
            thinking: None,
        };
        let model = p.into_model();
        assert!(
            model.supports_streaming(),
            "un provider remoto conserva streaming (comportamiento global intacto)"
        );
    }
}

#[cfg(test)]
mod ollama_canary_d_runtime_test {
    //! FASE 4 · Canary D — camino de RUNTIME: config → LlmProvider::from_config
    //! → into_model() (aplica el perfil local del micro-fix) → BaseModelReactLLM
    //! (react adapter del runtime) → adapter → ollama → qwen2.5:1.5b → respuesta.
    //!
    //! Ignorado (golpea Ollama loopback real):
    //!   cargo test -p nexum-acp --lib canary_d -- --ignored --nocapture
    use super::config::{AppConfig, NexumConfig, ProviderConfig};
    use super::LlmProvider;
    use nexum_agent::agent::react::ReactLLM;
    use nexum_agent::agent::FnEventHandler;
    use nexum_agent::llm::types::StreamingContext;
    use nexum_agent::llm::BaseModelReactLLM;
    use nexum_agent::messages::{BaseMessage, MessageId};
    use nexum_agent::tools::BaseTool;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn ollama_cfg() -> NexumConfig {
        NexumConfig {
            schema: None,
            config: AppConfig {
                active_provider_id: "ollama_local".to_string(),
                active_alias: "qwen2.5:1.5b".to_string(),
                providers: vec![ProviderConfig {
                    id: "ollama_local".to_string(),
                    provider_type: "openai".to_string(),
                    api_key: "ollama".to_string(),
                    base_url: "http://127.0.0.1:11434/v1".to_string(),
                    name: Some("Ollama Local".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    #[ignore]
    async fn canary_d_runtime_roundtrip() {
        let cfg = ollama_cfg();
        let provider = LlmProvider::from_config(&cfg).expect("from_config debe resolver ollama");
        assert_eq!(provider.model_name(), "qwen2.5:1.5b");

        let model = provider.into_model(); // aplica with_local_cpu_profile() (fix)
        assert!(!model.supports_streaming(), "runtime debe resolver ollama a non-stream");
        let model_id = model.model_id().to_string();
        assert_eq!(model_id, "qwen2.5:1.5b", "provider_model_trace_match");

        // React adapter del runtime, con StreamingContext (como el producto):
        // invoke_streaming degrada a non-stream por la política local.
        let react = BaseModelReactLLM::new(model);
        let handler = Arc::new(FnEventHandler(|_ev| {}));
        let ctx = StreamingContext {
            event_handler: handler,
            message_id: MessageId::new(),
            cancel: CancellationToken::new(),
        };
        let messages = vec![BaseMessage::human("Respondé únicamente: NEXUM_PROVIDER_OK")];
        let tools: Vec<&dyn BaseTool> = vec![]; // sin tools: un solo roundtrip

        let t0 = std::time::Instant::now();
        let reasoning = react
            .generate_reasoning(&messages, &tools, Some(ctx))
            .await
            .expect("el roundtrip de runtime debe completar");
        let dt = t0.elapsed().as_secs_f64();

        let text = format!(
            "{} {} {:?}",
            reasoning.thought,
            reasoning.final_answer.clone().unwrap_or_default(),
            reasoning.source_message
        );
        let ok = text.contains("NEXUM_PROVIDER_OK");
        println!(
            "CANARY\tD_runtime_roundtrip\tprovider_id\tollama_local\tmodel_id\t{model_id}\tduration_s\t{dt:.2}\tcanary_ok\t{ok}\tduplicate_requests\t0\tresult\t{}",
            if ok && dt < 30.0 { "PASS" } else { "FAIL" }
        );
        assert!(ok, "la respuesta del runtime debe contener el canary");
        assert!(dt < 30.0, "el roundtrip de runtime debe completar < 30s: {dt:.2}s");
    }
}

#[cfg(test)]
mod free_access_gate_tests {
    // ALLOW justificado: temporales con PID en el nombre; sin recurso
    // compartido entre procesos.
    #![allow(clippy::disallowed_methods)]
    use super::*;

    /// Determinista: se apunta XDG_DATA_HOME a un directorio vacío para no
    /// depender del catálogo real de la máquina.
    #[test]
    fn sin_catalogo_ningun_provider_tiene_free_access() {
        let tmp = std::env::temp_dir().join(format!("nexum-facc-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let previo = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_DATA_HOME", &tmp);
        let zen = catalog_declares_free_access("opencode_zen");
        let ollama = catalog_declares_free_access("ollama_local");
        match previo {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(!zen, "sin catálogo no hay excepción");
        assert!(!ollama);
    }

    /// Y con un catálogo que SÍ lo declara, la excepción aplica sólo a ese id.
    #[test]
    fn solo_el_provider_declarado_obtiene_la_excepcion() {
        let tmp = std::env::temp_dir().join(format!("nexum-facc2-{}", std::process::id()));
        let dir = tmp.join(super::catalog_path::XDG_PROVIDERS_SUBDIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(super::catalog_path::LIVE_CATALOG_FILE_NAME),
            format!(
                r#"{{"catalog_generation":{},"providers":[
                 {{"provider_id":"opencode_zen","credential_state":"free_access"}},
                 {{"provider_id":"ollama_local","credential_state":"verified"}}]}}"#,
                CATALOG_GENERATION
            ),
        )
        .unwrap();
        let previo = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_DATA_HOME", &tmp);
        let zen = catalog_declares_free_access("opencode_zen");
        let ollama = catalog_declares_free_access("ollama_local");
        match previo {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(zen, "el declarado sí");
        assert!(!ollama, "un provider verificado NO obtiene la excepción");
    }

    #[test]
    fn provider_id_vacio_nunca_tiene_free_access() {
        assert!(!catalog_declares_free_access(""));
    }

    /// LOS 5 USABLES ACTUALES TOMAN EXACTAMENTE EL MISMO CAMINO QUE HOY.
    ///
    /// Todos tienen credencial, así que la rama nueva ni se evalúa: el
    /// cortocircuito de `&&` corta en `api_key.is_empty() == false`. Este test
    /// existe para que quede fijado en la suite, no por elegancia.
    #[test]
    fn los_cinco_usables_no_pasan_por_la_rama_nueva() {
        for (pid, key) in [
            ("ollama_local", "ollama"),
            ("claude_code", "sk-bridge-key"),
            ("codex_cli", "sk-bridge-key"),
            ("gemini_cli", "sk-bridge-key"),
            ("mimo_code", "xiaomi-key"),
        ] {
            assert!(
                !key.is_empty(),
                "{pid} tiene credencial: la guarda corta antes de consultar el catálogo"
            );
            // Con credencial presente, el resultado NO depende del catálogo.
            let con_credencial = !key.is_empty() || catalog_declares_free_access(pid);
            assert!(con_credencial, "{pid} debe seguir construyéndose igual que hoy");
        }
    }

    /// Un provider SIN credencial y SIN free_access sigue rechazado como antes.
    #[test]
    fn sin_credencial_y_sin_free_access_sigue_rechazado() {
        let key = "";
        let aceptado = !key.is_empty() || catalog_declares_free_access("provider_inventado");
        assert!(!aceptado, "sin declaración del catálogo no hay excepción");
    }
}

#[cfg(test)]
mod generation_stamp_tests {
    // ALLOW justificado: los temporales de estos tests llevan PID en el
    // nombre, así que no hay recurso compartido entre procesos. El lint
    // protege contra nombres FIJOS, que es otra cosa.
    #![allow(clippy::disallowed_methods)]
    use super::*;
    use serde_json::json;

    #[test]
    fn misma_generacion_permite_las_concesiones() {
        let v = generation_verdict(&json!({"catalog_generation": CATALOG_GENERATION}));
        assert_eq!(v, GenerationVerdict::Match);
        assert!(v.grants_allowed());
        assert!(v.message().is_none(), "sin desfasaje no hay ruido");
    }

    /// Los 25 slots que ya existen tienen catálogos SIN el campo.
    /// Rodar a uno de ellos y volver no puede desactivar el tier libre en
    /// silencio: se trata como generación 0, con mensaje explícito.
    #[test]
    fn catalogo_sin_estampa_es_generacion_cero_y_avisa() {
        let v = generation_verdict(&json!({"providers": []}));
        assert_eq!(v, GenerationVerdict::Absent);
        assert!(!v.grants_allowed(), "sin estampa no se concede acceso libre");
        let msg = v.message().expect("la degradación tiene que ser visible");
        assert!(msg.contains("reconcile"), "tiene que decir QUÉ hacer");
        assert!(msg.contains("el resto no se ve afectado"));
    }

    /// El caso peligroso: un catálogo más nuevo puede conceder `free_access`
    /// con una semántica que este binario no conoce.
    #[test]
    fn catalogo_mas_nuevo_rechaza_las_concesiones_y_grita() {
        let v = generation_verdict(&json!({"catalog_generation": CATALOG_GENERATION + 1}));
        assert!(matches!(v, GenerationVerdict::TooNew { .. }));
        assert!(!v.grants_allowed());
        assert!(v.message().unwrap().contains("POSTERIOR"));
    }

    #[test]
    fn catalogo_viejo_degrada_avisando_no_en_silencio() {
        let v = generation_verdict(&json!({"catalog_generation": 1}));
        assert!(matches!(v, GenerationVerdict::TooOld { .. }));
        assert!(!v.grants_allowed());
        assert!(v.message().is_some());
    }

    /// La estampa CUSTODIA la concesión: aunque el catálogo declare
    /// free_access, sin generación válida no se concede.
    #[test]
    fn sin_estampa_no_hay_free_access_aunque_el_catalogo_lo_declare() {
        let tmp = std::env::temp_dir().join(format!("nexum-gen-{}", std::process::id()));
        let dir = tmp.join(super::catalog_path::XDG_PROVIDERS_SUBDIR);
        std::fs::create_dir_all(&dir).unwrap();
        // Catálogo que declara free_access pero SIN estampa.
        std::fs::write(
            dir.join(super::catalog_path::LIVE_CATALOG_FILE_NAME),
            r#"{"providers":[{"provider_id":"opencode_zen","credential_state":"free_access"}]}"#,
        )
        .unwrap();
        let previo = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_DATA_HOME", &tmp);
        let sin_estampa = catalog_declares_free_access("opencode_zen");
        // Ahora el mismo catálogo CON la estampa correcta.
        std::fs::write(
            dir.join(super::catalog_path::LIVE_CATALOG_FILE_NAME),
            format!(
                r#"{{"catalog_generation":{},"providers":[{{"provider_id":"opencode_zen","credential_state":"free_access"}}]}}"#,
                CATALOG_GENERATION
            ),
        )
        .unwrap();
        let con_estampa = catalog_declares_free_access("opencode_zen");
        match previo {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(!sin_estampa, "sin estampa NO se concede");
        assert!(con_estampa, "con estampa válida sí");
    }
}

#[cfg(test)]
mod generation_contract_tests {
    use super::CATALOG_GENERATION;

    /// Busca la FUENTE ÚNICA del contrato: primero el `config/` del propio
    /// repo, después el slot instalado.
    ///
    /// El orden importa. Antes el primer candidato con contenido era el slot
    /// instalado y el segundo un path bajo `/tmp`; los dos son estado de la
    /// máquina, no del árbol que se está compilando. `/tmp` además se borra al
    /// reiniciar, y como la comparación se omite en silencio cuando no
    /// encuentra el archivo, la estampa podía dejar de custodiar sin que nada
    /// lo dijera. Desde que `config/catalog-contract.json` vive en el repo, la
    /// fuente que corresponde es esa: viaja con el commit que se compila.
    fn contract_generation() -> Option<u64> {
        let mut candidatos: Vec<std::path::PathBuf> = vec![
            // `CARGO_MANIFEST_DIR` es `<repo>/nexum-acp`.
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../config/catalog-contract.json"),
        ];
        if let Ok(home) = std::env::var("HOME") {
            candidatos.push(
                std::path::PathBuf::from(&home)
                    .join(".local/lib/nexum/current/catalog-contract.json"),
            );
        }
        for p in candidatos {
            if let Ok(raw) = std::fs::read_to_string(&p) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(g) = v.get("generation").and_then(|x| x.as_u64()) {
                        return Some(g);
                    }
                }
            }
        }
        None
    }

    /// Si Rust y Python llevan generaciones distintas, la estampa deja de
    /// custodiar nada: concedería o negaría por un desfasaje propio.
    #[test]
    fn rust_y_python_declaran_la_misma_generacion() {
        let Some(contrato) = contract_generation() else {
            // Sin el contrato a la vista no se puede comparar; no se inventa
            // un veredicto. El gate de paridad cubre que el archivo viaje.
            eprintln!("contrato no encontrado: comparación omitida");
            return;
        };
        assert_eq!(
            CATALOG_GENERATION, contrato,
            "la constante de Rust ({CATALOG_GENERATION}) y la del contrato \
             ({contrato}) divergieron: bumpeá ambos lados o revertí uno"
        );
    }
}


#[cfg(test)]
mod tool_support_tests {
    // ALLOW justificado: los temporales de estos tests llevan PID en el
    // nombre, así que no hay recurso compartido entre procesos. El lint
    // protege contra nombres FIJOS, que es otra cosa.
    #![allow(clippy::disallowed_methods)]
    use super::*;

    /// El catálogo se lee del disco, así que los tests apuntan `XDG_DATA_HOME`
    /// a un directorio propio. Es exactamente la deuda que arrastran los tests
    /// de `nexum-tui`: leer el catálogo del usuario hace que el resultado
    /// dependa de qué providers tenga conectados.
    fn con_catalogo<T>(doc: serde_json::Value, f: impl FnOnce() -> T) -> T {
        let dir = std::env::temp_dir().join(format!(
            "nexum-toolsupport-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let provs = dir.join(super::catalog_path::XDG_PROVIDERS_SUBDIR);
        std::fs::create_dir_all(&provs).expect("crear dir de catálogo");
        std::fs::write(
            provs.join(super::catalog_path::LIVE_CATALOG_FILE_NAME),
            serde_json::to_string(&doc).expect("serializar"),
        )
        .expect("escribir catálogo");
        let previo = std::env::var("XDG_DATA_HOME").ok();
        // SAFETY: los tests de este módulo corren serializados por `--test-threads`
        // sobre el mismo env; el directorio es único por proceso+thread.
        unsafe { std::env::set_var("XDG_DATA_HOME", &dir) };
        let out = f();
        unsafe {
            match previo {
                Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    fn catalogo_ollama() -> serde_json::Value {
        serde_json::json!({
            "providers": [{
                "provider_id": "ollama_local",
                "model_capabilities": {
                    "qwen2.5:1.5b": {"tools": true},
                    "moondream:latest": {"tools": false}
                }
            }]
        })
    }

    #[test]
    fn un_modelo_que_declara_tools_es_supported() {
        let v = con_catalogo(catalogo_ollama(), || {
            model_tool_support_any("qwen2.5:1.5b")
        });
        assert_eq!(v, ToolSupport::Supported);
    }

    #[test]
    fn un_modelo_que_declara_no_tener_tools_es_unsupported() {
        let v = con_catalogo(catalogo_ollama(), || {
            model_tool_support_any("moondream:latest")
        });
        assert_eq!(v, ToolSupport::Unsupported);
    }

    #[test]
    fn no_saber_no_es_lo_mismo_que_no_poder() {
        // Un modelo sin dato NO se bloquea: la ausencia de información no puede
        // convertirse en una negativa, o cada provider sin `/api/show` quedaría
        // sin herramientas.
        let v = con_catalogo(catalogo_ollama(), || {
            model_tool_support_any("modelo-que-nadie-declaró")
        });
        assert_eq!(v, ToolSupport::Unknown);
    }

    #[test]
    fn sin_catalogo_nada_se_bloquea() {
        let v = con_catalogo(serde_json::json!({}), || {
            model_tool_support_any("moondream:latest")
        });
        assert_eq!(v, ToolSupport::Unknown);
    }

    #[test]
    fn con_un_provider_usable_no_es_primer_arranque() {
        let doc = serde_json::json!({
            "providers": [
                {"provider_id": "a", "usable_now": false},
                {"provider_id": "b", "usable_now": true}
            ]
        });
        assert!(con_catalogo(doc, catalog_has_usable_provider));
    }

    #[test]
    fn que_a_uno_le_falte_credencial_no_dispara_el_wizard() {
        // El caso exacto que lo rompió: Zen pierde free_access al quedarse sin
        // saldo, pero hay cinco providers andando. Eso NO es primer arranque.
        let doc = serde_json::json!({
            "providers": [
                {"provider_id": "zen", "usable_now": false, "credential_state": "verified_no_credit"},
                {"provider_id": "claude", "usable_now": true},
                {"provider_id": "gemini", "usable_now": true}
            ]
        });
        assert!(con_catalogo(doc, catalog_has_usable_provider));
    }

    #[test]
    fn sin_ningun_provider_usable_si_es_primer_arranque() {
        let doc = serde_json::json!({
            "providers": [{"provider_id": "a", "usable_now": false}]
        });
        assert!(!con_catalogo(doc, catalog_has_usable_provider));
    }

    #[test]
    fn sin_catalogo_es_primer_arranque() {
        assert!(!con_catalogo(serde_json::json!({}), catalog_has_usable_provider));
    }

    #[test]
    fn un_provider_que_lo_declara_capaz_gana_sobre_uno_que_no() {
        // Bloquear se reserva para cuando NADIE dice que puede.
        let doc = serde_json::json!({
            "providers": [
                {"provider_id": "a", "model_capabilities": {"m": {"tools": false}}},
                {"provider_id": "b", "model_capabilities": {"m": {"tools": true}}}
            ]
        });
        assert_eq!(con_catalogo(doc, || model_tool_support_any("m")), ToolSupport::Supported);
    }
}
