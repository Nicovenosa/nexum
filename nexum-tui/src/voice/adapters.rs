//! Adapters e interfaces preparadas para F2+ (solo mocks/stubs en F1).
//! Contratos completos: docs/architecture/interfaces/Voice*.md.

/// ASR (F2: WhisperCppAdapter; F3+: Parakeet tras benchmark). F1: mock.
pub trait AsrAdapter: Send {
    fn engine_name(&self) -> &'static str;
    fn is_real(&self) -> bool;
    /// F1: los mocks devuelven la fixture; jamás abren el micrófono.
    fn transcribe_fixture(&mut self, fixture: &str) -> String;
}

pub struct MockAsrAdapter;
impl AsrAdapter for MockAsrAdapter {
    fn engine_name(&self) -> &'static str {
        "mock-asr"
    }
    fn is_real(&self) -> bool {
        false
    }
    fn transcribe_fixture(&mut self, fixture: &str) -> String {
        fixture.to_string()
    }
}

/// TTS (F2: PiperAdapter es_AR-daniela-high opcional). F1: mock/disabled.
pub trait TtsAdapter: Send {
    fn engine_name(&self) -> &'static str;
    fn is_real(&self) -> bool;
    /// F1: no emite audio; devuelve qué HABRÍA dicho (para test/nota).
    fn speak(&mut self, text: &str) -> Option<String>;
}

pub struct MockTtsAdapter;
impl TtsAdapter for MockTtsAdapter {
    fn engine_name(&self) -> &'static str {
        "mock-tts"
    }
    fn is_real(&self) -> bool {
        false
    }
    fn speak(&mut self, text: &str) -> Option<String> {
        Some(text.to_string())
    }
}

/// Overlay (invariante futuro: Listening ⇒ overlay visible). F1: los
/// estados se registran para mostrarlos en la nota de /voz test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayState {
    PreparingRuntime,
    ReconnectingRuntime,
    RuntimeUnavailable,
    Listening,
    SpeechDetected,
    SilenceCountdown,
    Transcribing,
    LocalHandling,
    Escalating,
    Speaking,
    Error,
    PrivacyBlocked,
}

pub trait VoiceOverlay: Send {
    fn show(&mut self, state: OverlayState);
}

/// F1: acumula la secuencia de estados (simulación verificable en tests).
#[derive(Default)]
pub struct LogOverlay {
    pub seen: Vec<OverlayState>,
}
impl VoiceOverlay for LogOverlay {
    fn show(&mut self, state: OverlayState) {
        self.seen.push(state);
    }
}

/// Hotkey global (F2: KDE Custom Shortcut → `nexum voice --toggle`).
/// F1: SOLO configuración declarada, sin registro real.
pub struct VoiceHotkeyManager;
impl VoiceHotkeyManager {
    /// Lee la config sin registrar nada (F1 no toca el sistema).
    pub fn configured_hotkey() -> String {
        std::env::var("NEXUM_VOICE_HOTKEY").unwrap_or_else(|_| "Super+Z".to_string())
    }
}

/// Privacidad (requisitos duros del diseño). F1: todo local/mock.
pub struct VoicePrivacyPolicy;
impl VoicePrivacyPolicy {
    pub fn status_lines() -> Vec<&'static str> {
        vec![
            "modo: local/mock — sin micrófono real en F1",
            "audio a cloud: NUNCA (no existe adapter)",
            "grabación: ninguna (no se captura audio)",
            "transcripts: no se persisten",
            "push-to-talk: default (F2); always-listening: fuera de alcance",
        ]
    }
}
