//! Voice — cliente de voz headless del runtime Nexum (F2.x real).
//!
//! ADR-NEXUM-VOICE-AS-RUNTIME-CLIENT: la voz es un cliente del runtime
//! (Hormiguero primero, jamás automation de TUI/xdotool). Desde F2 el
//! pipeline es real: captura por pw-record/parecord/arecord, ASR
//! whisper.cpp, TTS Piper (default) o Kokoro (opcional), HUD v2 y turno
//! ACP (acp_turn). Los adapters mock de F1 (adapters.rs) quedan como capa
//! de testing usada por /voz test — no participan del flujo productivo.
//! Cloud sigue prohibido: todo local, allow_paid_ai=false en headless.
//!
//! # La capa de carácter por reescritura está DESCARTADA — no postergada
//!
//! **No la reintentes ajustando el prompt.** Se midió el 2026-08-02 con
//! Qwen3-0.6B (`ollama`, `think=false`, `num_predict=40`) en el equipo objetivo
//! y falla por corrección, no por costo. Con un prompt de carácter *fuerte*,
//! con ejemplos few-shot, sobre frases que reportaban un fallo:
//!
//! ```text
//! "No pude borrar el archivo."   ->  "Listo, quedó."          invierte
//! "La build rompió en 3 tests."  ->  "Listo, quedó."          invierte
//! "No encontré ese modelo."      ->  "Listo, quedó."          invierte
//! "El comando falló."            ->  "Se cayó la conexión."   FABRICA la causa
//! ```
//!
//! 4 de 6. El modelo copia los ejemplos del prompt en vez de aplicar la
//! transformación —comportamiento típico de un modelo chico— así que **un
//! prompt mejor empeora el problema**: cuantos más ejemplos, más copia.
//!
//! Un asistente de voz que dice "listo, quedó" cuando el comando falló manda al
//! usuario a seguir trabajando sobre algo roto. Y **no se arregla mandándolo a
//! narración**: invertir un error leyendo texto largo es igual de grave.
//!
//! Latencia, que es lo de menos acá: mediana 471 ms en caliente, 3094 ms en
//! frío. Sumada a los 1060 ms de Piper da 1531 ms contra un techo de 1500.
//!
//! Evidencia completa: `docs/voice/MEDICION-CAPA-CARACTER-QWEN3.md`.
//!
//! ## Dónde va la identidad, entonces
//!
//! Si el carácter no puede venir de **reescribir la salida**, tiene que venir de
//! que Nexum **ya escriba así**: en el prompt de sistema del modelo principal,
//! que sí tiene capacidad para sostener un registro.
//!
//! Es estrictamente mejor por tres razones, no un premio consuelo:
//!
//! - **No agrega latencia.** El texto ya se genera; sale con carácter de una.
//! - **No puede invertir un mensaje.** No hay una segunda pasada que reinterprete
//!   un resultado ya decidido — el modelo que sabe que el comando falló es el
//!   mismo que redacta la frase.
//! - **Es más barato**: cero procesos, cero modelos extra, cero RAM.
//!
//! El alcance de esa capa es el REGISTRO —cómo frasea— nunca el contenido.
//! **Propuesto, no implementado**: no es trabajo de este módulo, y es decisión
//! de producto dónde vive ese prompt.
//!
//! Lo que NO cambia: `docs/voice/RFC-ARQUITECTURA-VOZ.md` sigue siendo la
//! autoridad de qué motor habla; esto es sobre qué se dice, no sobre cómo suena.

pub mod adapters;
pub mod acp_turn;
pub mod asr_whisper;
pub mod audio;
pub mod bootstrap;
pub mod catalog;
pub mod descripcion;
pub mod doctor;
pub mod headless;
pub mod hud;
pub mod hud_model;
pub mod model_directive;
pub mod onboarding;
pub mod profile;
pub mod runtime;
pub mod vad;
pub mod intent_router;
pub mod overlay_notify;
pub mod tts_kokoro;
pub mod tts_backend;
pub mod tts_piper;

#[cfg(test)]
mod acp_turn_test;

#[cfg(test)]
mod omega_e2e_test;

pub use adapters::*;
pub use intent_router::VoiceIntentRouter;

/// Flag de voz (gobierna la FUTURA captura real; el pipeline mock de
/// `/voz test` es inocuo y corre siempre). Default OFF; public demo gana.
pub fn voice_enabled() -> bool {
    if crate::ui::demo_mode::public_demo_enabled() {
        return false;
    }
    matches!(
        std::env::var("NEXUM_VOICE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "on" | "1" | "true" | "yes"
    )
}

/// Cómo se resolvió el turno de voz (contrato del spec F1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandledBy {
    Local,
    PaidAi,
    Blocked,
}

impl std::fmt::Display for HandledBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandledBy::Local => write!(f, "local"),
            HandledBy::PaidAi => write!(f, "paid_ai"),
            HandledBy::Blocked => write!(f, "blocked"),
        }
    }
}

/// Hint para el HUD v2 (F2.3): estado especial a mostrar tras el turno
/// (además de handled_by). `None` = flujo estándar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HudHint {
    #[default]
    None,
    VoiceChanged,
    ModelChanged,
    WaitingConfirmation,
}

/// Respuesta de un turno de voz (contrato del spec F1 + hud_hint F2.3).
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceResponse {
    pub text_full: String,
    pub text_speakable: String,
    pub handled_by: HandledBy,
    pub should_speak: bool,
    pub should_show_in_tui: bool,
    pub hud_hint: HudHint,
}

impl VoiceResponse {
    /// Turno resuelto local que se habla y se muestra (constructor común).
    pub fn local(msg: String, hint: HudHint) -> Self {
        Self {
            text_speakable: make_speakable(&msg),
            text_full: msg,
            handled_by: HandledBy::Local,
            should_speak: true,
            should_show_in_tui: true,
            hud_hint: hint,
        }
    }
}

/// speakable = corto, una idea, sin markdown ni valores redactados
/// pronunciables. Límite duro para no leer parrafadas.
pub fn make_speakable(full: &str) -> String {
    let no_md: String = full
        .replace("[REDACTED]", "un dato protegido")
        .chars()
        .filter(|c| !matches!(c, '`' | '*' | '#' | '|'))
        .collect();
    let first = no_md
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut out: String = first.chars().take(140).collect();
    if first.chars().count() > 140 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::{
        runtime::{OnboardingDeliveryWarning, TextDeliveryWarning, VoiceRuntime},
        tts_backend::FakeTtsBackend,
    };

    #[test]
    fn test_voice_flag_off_default_y_public_demo_gana() {
        let _g = crate::ui::demo_mode::test_env_lock();
        std::env::remove_var("NEXUM_VOICE");
        std::env::remove_var("NEXUM_PUBLIC_DEMO");
        assert!(!voice_enabled(), "default OFF");
        std::env::set_var("NEXUM_VOICE", "on");
        assert!(voice_enabled());
        std::env::set_var("NEXUM_PUBLIC_DEMO", "1");
        assert!(!voice_enabled(), "public demo nunca activa voz real");
        std::env::remove_var("NEXUM_PUBLIC_DEMO");
        std::env::remove_var("NEXUM_VOICE");
    }

    #[test]
    fn test_speakable_corto_sin_markdown_ni_secretos() {
        let full = "## `código` con [REDACTED] y más texto\nsegunda línea";
        let s = make_speakable(full);
        assert!(!s.contains('#') && !s.contains('`'));
        assert!(s.contains("un dato protegido"));
        let largo = "x".repeat(500);
        assert!(make_speakable(&largo).chars().count() <= 141);
    }

    #[test]
    fn test_runtime_entrega_texto_antes_de_tts_opcional() {
        let response = VoiceResponse::local("respuesta visible".into(), HudHint::None);
        let mut runtime = VoiceRuntime::enabled(Some(FakeTtsBackend::default()));
        let delivery = runtime.deliver(&response);
        assert_eq!(delivery.text, "respuesta visible");
        assert!(delivery.spoken);
        assert_eq!(delivery.warning, None);
        assert_eq!(runtime.backend().unwrap().spoken(), &["respuesta visible"]);
    }

    #[test]
    fn test_runtime_conserva_texto_ante_error_cancelacion_y_flag_desactivado() {
        let response = VoiceResponse::local("respuesta visible".into(), HudHint::None);
        let mut failing = VoiceRuntime::enabled(Some(FakeTtsBackend::failing("sin salida")));
        let failed = failing.deliver(&response);
        assert_eq!(failed.text, "respuesta visible");
        assert_eq!(failed.warning, Some(TextDeliveryWarning::TtsUnavailable("sin salida".into())));

        let mut cancelled = VoiceRuntime::<FakeTtsBackend>::enabled(None);
        cancelled.cancel();
        let cancelled_delivery = cancelled.deliver(&response);
        assert_eq!(cancelled_delivery.text, "respuesta visible");
        assert_eq!(cancelled_delivery.warning, Some(TextDeliveryWarning::Cancelled));

        let mut disabled = VoiceRuntime::<FakeTtsBackend>::disabled();
        let disabled_delivery = disabled.deliver(&response);
        assert_eq!(disabled_delivery.text, "respuesta visible");
        assert_eq!(disabled_delivery.warning, Some(TextDeliveryWarning::Disabled));
    }

    #[test]
    fn test_runtime_entrega_addendum_estructurado_despues_del_texto_una_sola_vez() {
        let dir = tempfile::tempdir().unwrap();
        let profile_path = dir.path().join("voice/profile.json");
        let response = VoiceResponse::local("respuesta visible".into(), HudHint::None);
        let mut runtime = VoiceRuntime::<FakeTtsBackend>::enabled(None);

        let first = runtime.deliver_with_onboarding(&response, "es-AR", &profile_path);
        assert_eq!(first.text.text, "respuesta visible");
        let addendum = first.addendum.expect("el texto precede al addendum");
        assert_eq!(addendum.path, onboarding::ACP_SESSION_PROMPT_PATH);
        assert!(addendum.text.contains("nexum_default"));
        assert_eq!(profile::load_from_path(&profile_path).unwrap().unwrap().id, "nexum_default");

        let second = runtime.deliver_with_onboarding(&response, "es-AR", &profile_path);
        assert_eq!(second.text.text, "respuesta visible");
        assert_eq!(second.addendum, None, "el addendum se entrega una sola vez");
    }

    #[test]
    fn test_runtime_no_oculta_texto_con_perfil_corrupto_cancelado_o_desactivado() {
        let dir = tempfile::tempdir().unwrap();
        let corrupt_path = dir.path().join("profile.json");
        std::fs::write(&corrupt_path, "{perfil invalido").unwrap();
        let response = VoiceResponse::local("respuesta visible".into(), HudHint::None);

        let mut corrupt = VoiceRuntime::<FakeTtsBackend>::enabled(None);
        let corrupt_delivery = corrupt.deliver_with_onboarding(&response, "es-AR", &corrupt_path);
        assert_eq!(corrupt_delivery.text.text, "respuesta visible");
        assert_eq!(corrupt_delivery.addendum, None);
        assert!(matches!(
            corrupt_delivery.onboarding_warning,
            Some(OnboardingDeliveryWarning::ProfileCorrupt(_))
        ));

        let mut cancelled = VoiceRuntime::<FakeTtsBackend>::enabled(None);
        cancelled.cancel();
        let cancelled_delivery = cancelled.deliver_with_onboarding(&response, "es-AR", &corrupt_path);
        assert_eq!(cancelled_delivery.text.text, "respuesta visible");
        assert_eq!(cancelled_delivery.onboarding_warning, Some(OnboardingDeliveryWarning::Cancelled));

        let mut disabled = VoiceRuntime::<FakeTtsBackend>::disabled();
        let disabled_delivery = disabled.deliver_with_onboarding(&response, "es-AR", &corrupt_path);
        assert_eq!(disabled_delivery.text.text, "respuesta visible");
        assert_eq!(disabled_delivery.onboarding_warning, Some(OnboardingDeliveryWarning::Disabled));
    }
}
