//! HUD v2 (F2.3): el HUD es el contenedor ÚNICO de la experiencia de voz.
//! Arquitectura en 3 capas: `HudPhase` (modelo de estados) → `frame_for`
//! (política de mensajes: texto, contexto, acento, duración) → `HudFrame`
//! (contrato JSON con el renderer QML). Las notificaciones del sistema
//! quedan SOLO como fallback cuando no hay ventana QML.
//!
//! Línea 1: estado ("Nexum escuchando…"). Línea 2: ruta/motores
//! ("Ruta activa: Hormiguero" · "Local: Hormiguero · Cloud: Codex").
//! Nada técnico ni ruidoso; debug opcional vía NEXUM_VOICE_HUD_MODE=debug.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudPhase {
    Idle,
    PreparingRuntime,
    ReconnectingRuntime,
    RuntimeUnavailable,
    Listening,
    SpeechDetected,
    SilenceCountdown,
    NoSpeech,
    Transcribing,
    LocalHandling,
    Escalating,
    Speaking,
    Success,
    Error,
    Fallback,
    VoiceChanged,
    ModelChanged,
    WaitingConfirmation,
    PrivacyBlocked,
}

/// Clasificación chica y honesta del modelo configurado. NUNCA "Cloud":
/// el modelo elegido puede ser perfectamente local (Ollama).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Local,
    Subscription,
    PaidApi,
}

impl std::fmt::Display for ModelKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelKind::Local => write!(f, "Local"),
            ModelKind::Subscription => write!(f, "Suscripción"),
            ModelKind::PaidApi => write!(f, "API de pago"),
        }
    }
}

/// Heurística: ollama/local = Local; loopback (puentes CLIProxyAPI de
/// suscripción) = Suscripción; resto (endpoint remoto con key) = API.
pub fn classify(p: &crate::config::ProviderConfig) -> ModelKind {
    let hay = format!("{} {}", p.id, p.name.as_deref().unwrap_or("")).to_lowercase();
    if hay.contains("ollama") || hay.contains("local") {
        ModelKind::Local
    } else if p.base_url.contains("127.0.0.1") || p.base_url.contains("localhost") {
        ModelKind::Subscription
    } else {
        ModelKind::PaidApi
    }
}

/// Modelo configurado = provider/model ELEGIDO (independiente de quién
/// procese el turno; eso es la ruta activa).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfiguredModel {
    pub label: String,
    pub kind: ModelKind,
}

/// Motores (línea 2 del HUD). `configured=None` = sin modelo elegido.
#[derive(Debug, Clone, Default)]
pub struct EnginesInfo {
    pub local: String,
    pub configured: Option<ConfiguredModel>,
}

impl EnginesInfo {
    pub fn detect() -> Self {
        let configured = crate::config::load().ok().and_then(|c| configured_model(&c.config));
        Self { local: "Hormiguero".into(), configured }
    }
}

/// Etiqueta corta y elegante del provider activo: "Codex · gpt-5".
pub fn configured_model(cfg: &crate::config::AppConfig) -> Option<ConfiguredModel> {
    let p = cfg.providers.iter().find(|p| p.id == cfg.active_provider_id)?;
    let name = short_provider_name(p.name.as_deref().unwrap_or(&p.id));
    let model = model_for_alias(p, &cfg.active_alias);
    let label = if model.is_empty() {
        format!("{name} · {}", cfg.active_alias)
    } else {
        format!("{name} · {model}")
    };
    Some(ConfiguredModel { label, kind: classify(p) })
}

/// "Codex / OpenAI (puente CLIProxyAPI)" → "Codex". Sin jerga de plomería.
pub fn short_provider_name(name: &str) -> String {
    let base = name.split(" (").next().unwrap_or(name);
    base.split(" / ").next().unwrap_or(base).trim().to_string()
}

pub fn model_for_alias(p: &crate::config::ProviderConfig, alias: &str) -> String {
    match alias {
        "opus" => p.models.opus.clone(),
        "sonnet" => p.models.sonnet.clone(),
        "haiku" => p.models.haiku.clone(),
        _ => String::new(),
    }
}

/// Ruta activa del turno = QUIÉN está procesando AHORA. La fija el
/// pipeline por fase (jamás se deriva sola de settings.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveRoute {
    #[default]
    Undecided,
    Local,
    Escalated,
}

#[derive(Debug, Clone, Default)]
pub struct HudContext {
    pub engines: EnginesInfo,
    pub route: ActiveRoute,
    pub debug: bool,
}

impl HudContext {
    /// Línea 2: ANTES del routing muestra el modelo configurado (con su
    /// clasificación chica); DESPUÉS muestra la ruta activa real.
    pub fn route_line(&self) -> String {
        match self.route {
            ActiveRoute::Undecided => match &self.engines.configured {
                Some(m) => format!("Modelo: {} — {}", m.label, m.kind),
                None => "Modelo: no configurado".into(),
            },
            ActiveRoute::Local => format!("Ruta activa: {}", self.engines.local),
            ActiveRoute::Escalated => match &self.engines.configured {
                Some(m) => format!("Ruta activa: {}", m.label),
                None => "Ruta activa: modelo no configurado".into(),
            },
        }
    }
}

/// Contrato con el renderer (JSON atómico que polea el QML).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HudFrame {
    pub seq: u32,
    pub state: &'static str,
    pub title: String,
    pub context: String,
    pub accent: &'static str,
    /// 0 = persistente hasta el próximo estado; >0 = fade-out elegante.
    pub ttl_ms: u32,
    pub pulse: bool,
    pub waveform: bool,
}

const VIOLET: &str = "#7c5cff"; // presencia/escucha
const CYAN: &str = "#25c8d8"; // local
const LILA: &str = "#b48ead"; // cloud/escalando
const GREEN: &str = "#4fd08a"; // éxito/cambios aplicados
const RED: &str = "#e0596e"; // error
const AMBER: &str = "#e0a34c"; // atención/fallback/confirmación

/// Política de mensajes + duración por estado. `detail` (opcional) refina
/// título o contexto según el estado (respuesta corta, voz nueva, modelo).
pub fn frame_for(phase: HudPhase, ctx: &HudContext, detail: Option<&str>) -> HudFrame {
    use HudPhase::*;
    let d = |fallback: &str| detail.unwrap_or(fallback).to_string();
    let route = ctx.route_line();
    let (state, title, context, accent, ttl_ms, pulse, waveform) = match phase {
        Idle => ("idle", "Nexum".into(), route, VIOLET, 1500, false, false),
        PreparingRuntime => ("preparing_runtime", "Preparando Nexum…".into(), route, VIOLET, 0, true, false),
        ReconnectingRuntime => ("reconnecting_runtime", "Reconectando…".into(), route, AMBER, 0, true, false),
        RuntimeUnavailable => (
            "runtime_unavailable",
            "Hormiguero no respondió".into(),
            "Super+Z para reintentar · nexum voice --repair".into(),
            AMBER, 10_000, false, false,
        ),
        Listening => ("listening", "Nexum escuchando…".into(), route, VIOLET, 0, true, false),
        SpeechDetected => ("speech_detected", "Te escucho…".into(), route, VIOLET, 0, true, true),
        SilenceCountdown => ("silence_countdown", "Cerrando escucha…".into(), route, VIOLET, 0, false, false),
        NoSpeech => (
            "no_speech",
            "No se detectó audio".into(),
            "Probá de nuevo con Super+Z".into(),
            AMBER, 5_000, false, false,
        ),
        Transcribing => ("transcribing", "Transcribiendo…".into(), route, VIOLET, 0, true, false),
        LocalHandling => ("local_handling", "Resolviendo localmente…".into(), route, CYAN, 0, true, false),
        Escalating => ("escalating", "Resolviendo consulta compleja…".into(), route, LILA, 0, true, false),
        Speaking => ("speaking", "Nexum respondiendo…".into(), route, VIOLET, 0, false, true),
        Success => ("success", d("Listo."), route, GREEN, 4_000, false, false),
        Error => ("error", d("Algo salió mal"), "Super+Z para reintentar".into(), RED, 6_000, false, false),
        Fallback => ("fallback", d("Usando plan B"), route, AMBER, 6_000, false, false),
        VoiceChanged => ("voice_changed", "Voz actualizada".into(), d(""), GREEN, 4_000, false, false),
        ModelChanged => ("model_changed", "Modelo actualizado".into(), d(""), GREEN, 4_000, false, false),
        WaitingConfirmation => (
            "waiting_confirmation",
            "Esperando tu confirmación…".into(),
            d("Decí «confirmo» o «cancelar»"),
            AMBER, 15_000, false, false,
        ),
        PrivacyBlocked => ("privacy_blocked", "Voz desactivada por privacidad".into(), route, AMBER, 6_000, false, false),
    };
    let context = if ctx.debug { format!("{context} [{state}]") } else { context };
    HudFrame { seq: 0, state, title, context, accent, ttl_ms, pulse, waveform }
}

/// Título apto para el HUD: primera idea, sin markdown, ≤72 chars.
pub fn hud_title(text: &str) -> String {
    let clean: String = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .chars()
        .filter(|c| !matches!(c, '`' | '*' | '#' | '|'))
        .collect();
    let t = clean.trim();
    if t.chars().count() <= 72 {
        t.to_string()
    } else {
        let cut: String = t.chars().take(69).collect();
        format!("{}…", cut.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(route: ActiveRoute, configured: Option<(&str, ModelKind)>) -> HudContext {
        HudContext {
            engines: EnginesInfo {
                local: "Hormiguero".into(),
                configured: configured.map(|(l, kind)| ConfiguredModel { label: l.into(), kind }),
            },
            route,
            debug: false,
        }
    }

    #[test]
    fn test_linea_2_modelo_configurado_vs_ruta_activa() {
        // ANTES del routing: modelo configurado con clasificación chica.
        let c = ctx(ActiveRoute::Undecided, Some(("Ollama Local · qwen3:1.7b", ModelKind::Local)));
        assert_eq!(c.route_line(), "Modelo: Ollama Local · qwen3:1.7b — Local");
        assert!(!c.route_line().contains("Cloud"), "jamás 'Cloud: Ollama'");
        let c = ctx(ActiveRoute::Undecided, Some(("Codex · gpt-5", ModelKind::Subscription)));
        assert_eq!(c.route_line(), "Modelo: Codex · gpt-5 — Suscripción");
        // DESPUÉS del routing: quién procesa el turno de verdad.
        assert_eq!(
            ctx(ActiveRoute::Local, Some(("Codex · gpt-5", ModelKind::Subscription))).route_line(),
            "Ruta activa: Hormiguero"
        );
        assert_eq!(
            ctx(ActiveRoute::Escalated, Some(("Codex · gpt-5", ModelKind::Subscription))).route_line(),
            "Ruta activa: Codex · gpt-5"
        );
        assert!(ctx(ActiveRoute::Undecided, None).route_line().contains("no configurado"));
    }

    #[test]
    fn test_classify_local_suscripcion_api() {
        use crate::config::ProviderConfig;
        let mk = |id: &str, name: &str, url: &str| ProviderConfig {
            id: id.into(),
            name: Some(name.into()),
            base_url: url.into(),
            ..Default::default()
        };
        assert_eq!(classify(&mk("ollama-local", "Ollama Local", "http://127.0.0.1:11434/v1")), ModelKind::Local);
        assert_eq!(classify(&mk("codex_cli", "Codex / OpenAI (puente CLIProxyAPI)", "http://127.0.0.1:8317/v1")), ModelKind::Subscription);
        assert_eq!(classify(&mk("deepseek", "DeepSeek", "https://api.deepseek.com/v1")), ModelKind::PaidApi);
    }

    #[test]
    fn test_politica_duraciones_y_acentos() {
        let c = ctx(ActiveRoute::Undecided, None);
        // Estados de proceso: persistentes (ttl 0).
        for p in [HudPhase::Listening, HudPhase::Transcribing, HudPhase::Escalating] {
            assert_eq!(frame_for(p, &c, None).ttl_ms, 0, "{p:?} persistente");
        }
        // Terminales: autodesaparición elegante.
        assert_eq!(frame_for(HudPhase::Success, &c, None).ttl_ms, 4_000);
        assert_eq!(frame_for(HudPhase::Error, &c, None).ttl_ms, 6_000);
        assert_eq!(frame_for(HudPhase::VoiceChanged, &c, None).ttl_ms, 4_000);
        assert_eq!(frame_for(HudPhase::LocalHandling, &c, None).accent, CYAN);
        assert_eq!(frame_for(HudPhase::Escalating, &c, None).accent, LILA);
        assert_eq!(frame_for(HudPhase::Error, &c, None).accent, RED);
    }

    #[test]
    fn test_detail_refina_titulo_o_contexto() {
        let c = ctx(ActiveRoute::Local, None);
        let f = frame_for(HudPhase::Success, &c, Some("La IP es 192.168.1.51"));
        assert_eq!(f.title, "La IP es 192.168.1.51");
        let f = frame_for(HudPhase::ModelChanged, &c, Some("Cloud: DeepSeek"));
        assert_eq!(f.context, "Cloud: DeepSeek");
        let f = frame_for(HudPhase::VoiceChanged, &c, Some("Nexum Claro · preferencia guardada"));
        assert!(f.context.contains("Nexum Claro"));
    }

    #[test]
    fn test_debug_agrega_state_id_y_normal_no() {
        let mut c = ctx(ActiveRoute::Local, None);
        assert!(!frame_for(HudPhase::Listening, &c, None).context.contains("[listening]"));
        c.debug = true;
        assert!(frame_for(HudPhase::Listening, &c, None).context.contains("[listening]"));
    }

    #[test]
    fn test_hud_title_corta_elegante_y_sin_markdown() {
        assert_eq!(hud_title("**Listo.**\ndetalle largo"), "Listo.");
        let largo = "palabra ".repeat(30);
        let t = hud_title(&largo);
        assert!(t.chars().count() <= 72 && t.ends_with('…'));
    }

    #[test]
    fn test_short_provider_name_elegante() {
        assert_eq!(short_provider_name("Codex / OpenAI (puente CLIProxyAPI)"), "Codex");
        assert_eq!(short_provider_name("Claude (puente CLIProxyAPI)"), "Claude");
        assert_eq!(short_provider_name("Ollama Local"), "Ollama Local");
    }
}
