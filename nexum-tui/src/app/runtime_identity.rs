//! runtime_identity — fuente ÚNICA de verdad de "¿qué proveedor+modelo está
//! activo ahora mismo?" (Sprint A, Bug 5).
//!
//! Regla: UI == runtime == respuesta a la pregunta. Los tres consumidores
//! obligatorios leen de acá:
//!   1. La statusbar inferior (`ui/main_ui/status_bar.rs`).
//!   2. El interceptor de preguntas de identidad en `submit_message`
//!      ("¿qué modelo estás usando?" se responde desde acá, sin pasar por el
//!      LLM — el LLM inventa su identidad).
//!   3. El welcome card del splash (mostraba la copia stale de services).
//!
//! La fuente es la `NexumConfig` compartida (Arc<RwLock<..>> que comparten TUI
//! y ACP server): `active_provider_id` + `active_alias` + `providers`. El
//! runtime resuelve el destino del prompt con `LlmProvider::from_config`
//! sobre la MISMA config, así que por construcción no puede divergir.

use crate::app::agent::LlmProvider;
use crate::config::NexumConfig;

/// Identidad del runtime en este instante.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeIdentity {
    /// Id interno del provider activo (ej. "opencode_zen", "ollama-local").
    pub provider_id: String,
    /// Nombre visible de la familia (ej. "OpenCode Free", "Ollama Local").
    pub provider_family: String,
    /// Modelo concreto que recibe los prompts (ej. "deepseek-v4-flash-free").
    pub model_id: String,
    /// Base URL del endpoint, si aplica (vacío para fallbacks sin config).
    pub base_url: String,
    /// De dónde salió la identidad:
    ///   - "user_selection": provider de la config (elegido/persistido por el
    ///     usuario vía /modelo o /provedor).
    ///   - "runtime": fallback por variables de entorno (sin config).
    ///   - "none": no hay proveedor utilizable.
    /// ("catalog" queda reservado para consumidores que lean el catálogo
    /// JSON directamente; el TUI siempre resuelve por config/runtime.)
    pub source: &'static str,
}

impl RuntimeIdentity {
    fn none() -> Self {
        Self {
            provider_id: String::new(),
            provider_family: String::new(),
            model_id: String::new(),
            base_url: String::new(),
            source: "none",
        }
    }
}

/// Resuelve la identidad activa desde la fuente única (NexumConfig compartida).
///
/// Usa exactamente el mismo camino que el runtime al mandar un prompt
/// (`LlmProvider::from_config(..).or_else(from_env)`), así que lo que devuelve
/// es, por construcción, lo que va a recibir el próximo prompt.
pub fn runtime_identity(cfg: &NexumConfig) -> RuntimeIdentity {
    if let Some(p) = LlmProvider::from_config(cfg) {
        let provider_cfg = cfg
            .config
            .providers
            .iter()
            .find(|pc| pc.id == cfg.config.active_provider_id);
        let family = crate::app::model_panel::provider_display_name(cfg)
            .or_else(|| provider_cfg.map(|pc| pc.display_name().to_string()))
            .unwrap_or_else(|| p.display_name().to_string());
        let base_url = match &p {
            LlmProvider::OpenAi { base_url, .. } => base_url.clone(),
            LlmProvider::Anthropic { base_url, .. } => base_url.clone().unwrap_or_default(),
        };
        return RuntimeIdentity {
            provider_id: cfg.config.active_provider_id.clone(),
            provider_family: family,
            model_id: p.model_name().to_string(),
            base_url,
            source: "user_selection",
        };
    }
    if let Some(p) = LlmProvider::from_env() {
        let base_url = match &p {
            LlmProvider::OpenAi { base_url, .. } => base_url.clone(),
            LlmProvider::Anthropic { base_url, .. } => base_url.clone().unwrap_or_default(),
        };
        return RuntimeIdentity {
            provider_id: String::new(),
            provider_family: p.display_name().to_string(),
            model_id: p.model_name().to_string(),
            base_url,
            source: "runtime",
        };
    }
    RuntimeIdentity::none()
}

/// Par (provider, modelo) para la statusbar. Vacíos si no hay proveedor.
pub fn statusbar_identity(cfg: &NexumConfig) -> (String, String) {
    match crate::app::model_panel::provider_display_name(cfg) {
        Some(provider) => (provider, cfg.config.active_alias.clone()),
        None => (
            "Provider Catalog ERROR".to_string(),
            "selección no validada".to_string(),
        ),
    }
}

// ─── Interceptor de preguntas de identidad ────────────────────────────────────

/// Normaliza para el matcheo: minúsculas y sin tildes/signos.
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' | 'ü' => 'u',
            _ => c,
        })
        .filter(|c| !matches!(c, '¿' | '?' | '¡' | '!' | ',' | '.'))
        .collect()
}

/// Patrones simples (keywords, no ML) que identifican preguntas sobre la
/// identidad del modelo/proveedor activo.
const IDENTITY_PATTERNS: &[&str] = &[
    "que modelo",
    "cual es tu modelo",
    "nombre de tu modelo",
    "modelo estas usando",
    "modelo usas",
    "modelo sos",
    "modelo eres",
    "que proveedor",
    "cual es tu proveedor",
    "quien sos",
    "quien eres",
    "que llm",
    "which model",
    "what model",
    "what llm",
    "who are you",
];

/// Guard de longitud: las preguntas de identidad son cortas. Prompts largos
/// que mencionan "qué modelo de datos…" no deben interceptarse.
const MAX_IDENTITY_PROMPT_CHARS: usize = 120;

/// True si el prompt es una pregunta sobre la identidad del modelo/proveedor.
pub fn is_identity_question(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.chars().count() > MAX_IDENTITY_PROMPT_CHARS
    {
        return false;
    }
    let norm = normalize(trimmed);
    IDENTITY_PATTERNS.iter().any(|p| norm.contains(p))
}

/// Respuesta honesta construida desde la identidad del runtime (formato
/// sugerido por el sprint). Nunca pasa por el LLM.
pub fn identity_response(id: &RuntimeIdentity) -> String {
    if id.source == "none" {
        return "No hay ningún proveedor de modelos configurado ahora mismo. \
                Abrí `/proveedor` para conectar uno."
            .to_string();
    }
    let mut out = format!(
        "Estoy corriendo con **{}** usando el modelo **{}**.",
        id.provider_family, id.model_id
    );
    if !id.base_url.is_empty() {
        out.push_str(&format!("\nBase URL: `{}`", id.base_url));
    }
    out.push_str(
        "\n\n_(Respuesta directa de Nexum desde `runtime_identity()` — \
         verificada contra la configuración activa, sin pasar por el LLM.)_",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, ProviderConfig};

    /// Candado de entorno + catálogo controlado, para los tests que leen el
    /// catálogo vivo a través de `provider_display_name`.
    ///
    /// Sin esto dependían del catálogo REAL de la máquina: pasaban porque acá
    /// hay `opencode_zen` conectado, y en una instalación limpia habrían dado
    /// `Provider Catalog ERROR`. Es el baseline móvil que `catalog_fixture`
    /// existe para eliminar, y que acá se había vuelto a colar.
    ///
    /// Y falta el candado era además una carrera real: `XDG_DATA_HOME` es
    /// estado del PROCESO y los tests son hilos. Mientras otro test aislaba su
    /// catálogo, éste leía el directorio ajeno y fallaba con
    /// `left: "Provider Catalog ERROR"`. Los escritores tomaban el candado; los
    /// lectores no, así que la invariante valía sólo de un lado.
    fn catalogo_de_prueba() -> (
        std::sync::MutexGuard<'static, ()>,
        crate::app::catalog_fixture::CatalogoAislado,
    ) {
        let env = crate::ui::demo_mode::test_env_lock();
        let cat = crate::app::catalog_fixture::CatalogoAislado::con(serde_json::json!({
            "providers": [
                {
                    "id": "opencode_zen",
                    "display_name": "OpenCode Free",
                    "usable_now": true,
                    "models": ["deepseek-v4-flash-free"],
                },
                {
                    "id": "ollama-local",
                    "display_name": "Ollama Local",
                    "usable_now": true,
                    "models": ["qwen2.5:0.5b"],
                },
            ]
        }));
        (env, cat)
    }

    fn cfg_with(provider_id: &str, alias: &str, providers: Vec<ProviderConfig>) -> NexumConfig {
        NexumConfig {
            schema: None,
            config: AppConfig {
                active_alias: alias.to_string(),
                active_provider_id: provider_id.to_string(),
                providers,
                ..Default::default()
            },
        }
    }

    fn zen_provider() -> ProviderConfig {
        ProviderConfig {
            id: "opencode_zen".to_string(),
            provider_type: "openai".to_string(),
            api_key: "sk-test-123".to_string(),
            base_url: "https://opencode.ai/zen/v1".to_string(),
            name: Some("OpenCode Zen".to_string()),
            ..Default::default()
        }
    }

    fn ollama_provider() -> ProviderConfig {
        ProviderConfig {
            id: "ollama-local".to_string(),
            provider_type: "openai".to_string(),
            api_key: "ollama".to_string(),
            base_url: "http://127.0.0.1:11434/v1".to_string(),
            name: Some("Ollama Local".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn identidad_refleja_provider_activo() {
        let (_env, _cat) = catalogo_de_prueba();
        let cfg = cfg_with(
            "opencode_zen",
            "deepseek-v4-flash-free",
            vec![ollama_provider(), zen_provider()],
        );
        let id = runtime_identity(&cfg);
        assert_eq!(id.provider_id, "opencode_zen");
        assert_eq!(id.provider_family, "OpenCode Free");
        assert_eq!(id.model_id, "deepseek-v4-flash-free");
        assert_eq!(id.base_url, "https://opencode.ai/zen/v1");
        assert_eq!(id.source, "user_selection");
    }

    #[test]
    fn cambiar_modelo_activo_actualiza_los_consumidores() {
        let (_env, _cat) = catalogo_de_prueba();
        // Integración: el MISMO cambio de config actualiza statusbar (par),
        // handler de identidad (texto) y runtime (from_config, que es lo que
        // usa submit_message) — porque los tres leen de la misma fuente.
        let mut cfg = cfg_with(
            "ollama-local",
            "qwen2.5:0.5b",
            vec![ollama_provider(), zen_provider()],
        );
        let (fam, model) = statusbar_identity(&cfg);
        assert_eq!(
            (fam.as_str(), model.as_str()),
            ("Ollama Local", "qwen2.5:0.5b")
        );
        assert!(identity_response(&runtime_identity(&cfg)).contains("qwen2.5:0.5b"));

        // El usuario elige un modelo de Zen en /modelo:
        cfg.config.active_provider_id = "opencode_zen".to_string();
        cfg.config.active_alias = "deepseek-v4-flash-free".to_string();

        let (fam, model) = statusbar_identity(&cfg);
        assert_eq!(fam, "OpenCode Free");
        assert_eq!(model, "deepseek-v4-flash-free");
        let resp = identity_response(&runtime_identity(&cfg));
        assert!(resp.contains("OpenCode Free") && resp.contains("deepseek-v4-flash-free"));
        // Y el runtime (el que decide a dónde va el prompt) coincide:
        match crate::app::agent::LlmProvider::from_config(&cfg) {
            Some(crate::app::agent::LlmProvider::OpenAi {
                model, base_url, ..
            }) => {
                assert_eq!(model, "deepseek-v4-flash-free");
                assert_eq!(base_url, "https://opencode.ai/zen/v1");
            }
            other => panic!("runtime no coincide: {:?}", other.is_some()),
        }
    }

    #[test]
    fn sin_provider_source_none() {
        // Sin providers y sin env vars de fallback no debería haber identidad
        // inventada. (Si el entorno de test tiene OPENAI_API_KEY, from_env
        // resuelve "runtime" — ambas son respuestas honestas, nunca "none"
        // con datos inventados.)
        let cfg = cfg_with("nada", "x", vec![]);
        let id = runtime_identity(&cfg);
        assert!(id.source == "none" || id.source == "runtime");
        if id.source == "none" {
            assert!(id.model_id.is_empty());
            assert!(identity_response(&id).contains("/proveedor"));
        }
    }

    #[test]
    fn matchea_preguntas_de_identidad() {
        for q in [
            "¿Qué modelo estás usando?",
            "que modelo sos",
            "¿cuál es tu modelo?",
            "respondé solo con el nombre de tu modelo, sin explicaciones",
            "which model are you?",
            "what model is this",
            "¿quién sos?",
            "¿qué proveedor usás?",
        ] {
            assert!(is_identity_question(q), "debería matchear: {q}");
        }
    }

    #[test]
    fn no_matchea_prompts_normales() {
        for q in [
            "refactorizá este archivo",
            "¿cómo funciona el scheduler de tokio?",
            "/modelo",
            "explicame qué modelo de datos conviene para esta app de inventario, con tablas, relaciones y ejemplos de queries SQL para el caso de uso completo",
            "",
        ] {
            assert!(!is_identity_question(q), "NO debería matchear: {q}");
        }
    }

    #[test]
    fn respuesta_incluye_familia_modelo_y_base_url() {
        let (_env, _cat) = catalogo_de_prueba();
        let cfg = cfg_with(
            "opencode_zen",
            "deepseek-v4-flash-free",
            vec![zen_provider()],
        );
        let resp = identity_response(&runtime_identity(&cfg));
        assert!(resp.contains("**OpenCode Free**"));
        assert!(resp.contains("**deepseek-v4-flash-free**"));
        assert!(resp.contains("https://opencode.ai/zen/v1"));
    }
}
