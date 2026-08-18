//! Overlay real (Voice F2): notificación desktop actualizable vía
//! notify-send (-p captura el id, -r lo reemplaza → una sola burbuja que
//! muta de estado). Fallback textual a stderr si no hay entorno gráfico.
//! Invariante de privacidad: NUNCA escuchar sin este indicador visible.

use super::adapters::{OverlayState, VoiceOverlay};
use std::process::Command;

pub struct NotificationOverlay {
    notif_id: Option<u32>,
    has_notify: bool,
}

impl NotificationOverlay {
    pub fn new() -> Self {
        let has_notify = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join("notify-send").is_file()))
            .unwrap_or(false);
        Self { notif_id: None, has_notify }
    }

    fn label(state: OverlayState) -> (&'static str, &'static str, i32) {
        // (texto, icono, timeout_ms) — 0 = persistente mientras escucha.
        match state {
            OverlayState::PreparingRuntime => ("Preparando Nexum…", "system-run", 0),
            OverlayState::ReconnectingRuntime => ("Reconectando…", "view-refresh", 0),
            OverlayState::RuntimeUnavailable => ("Hormiguero no respondió · Super+Z para reintentar", "dialog-warning", 12000),
            OverlayState::Listening => ("● Nexum escuchando…", "audio-input-microphone", 0),
            OverlayState::SpeechDetected => ("Te escucho…", "audio-input-microphone", 0),
            OverlayState::SilenceCountdown => ("Cerrando escucha…", "chronometer", 0),
            OverlayState::Transcribing => ("Transcribiendo…", "view-refresh", 0),
            OverlayState::LocalHandling => ("Resolviendo localmente…", "system-run", 4000),
            OverlayState::Escalating => ("Necesita modelo avanzado…", "network-transmit", 4000),
            OverlayState::Speaking => ("Nexum respondiendo…", "audio-volume-high", 4000),
            OverlayState::Error => ("No pude escuchar bien", "dialog-warning", 5000),
            OverlayState::PrivacyBlocked => ("Voz desactivada por privacidad", "security-high", 5000),
        }
    }

    fn send(&mut self, title: &str, body: &str, icon: &str, timeout_ms: i32) {
        if !self.has_notify {
            eprintln!("[voice] {title} {body}");
            return;
        }
        let mut cmd = Command::new("notify-send");
        cmd.args(["-a", "Nexum", "-i", icon, "-t", &timeout_ms.to_string(), "-p"]);
        if let Some(id) = self.notif_id {
            cmd.args(["-r", &id.to_string()]);
        }
        cmd.arg(title);
        if !body.is_empty() {
            cmd.arg(body);
        }
        if let Ok(out) = cmd.output() {
            if let Ok(id) = String::from_utf8_lossy(&out.stdout).trim().parse::<u32>() {
                self.notif_id = Some(id);
            }
        }
    }

    /// Mensaje final con la respuesta (texto visible, ya redactado).
    pub fn show_answer(&mut self, text: &str, handled_by: &str) {
        let title = match handled_by {
            "local" => "Nexum (local)",
            "blocked" => "Nexum",
            _ => "Nexum",
        };
        // Cuerpo acotado: la respuesta completa vive en TUI/log.
        let body: String = text.chars().take(300).collect();
        self.send(title, &body, "dialog-information", 8000);
    }
}

impl Default for NotificationOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceOverlay for NotificationOverlay {
    fn show(&mut self, state: OverlayState) {
        let (text, icon, timeout) = Self::label(state);
        self.send(text, "", icon, timeout);
    }
}
