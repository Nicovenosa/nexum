//! /voz — estado y prueba del cliente de voz mock (Voice F1).
//! Sin micrófono, sin ASR/TTS reales, sin hotkey: pipeline mock completo
//! sobre el pasillo Hormiguero. Jamás llama provider pago (/voz test usa
//! allow_paid_ai=false SIEMPRE).

use crate::{
    app::App,
    command::Command,
    voice::{
        self, AsrAdapter, HandledBy, LogOverlay, MockAsrAdapter, MockTtsAdapter, OverlayState,
        TtsAdapter, VoiceHotkeyManager, VoiceIntentRouter, VoiceOverlay, VoicePrivacyPolicy,
    },
};

pub struct VozCommand;

impl Command for VozCommand {
    fn name(&self) -> &str {
        "voz"
    }

    fn description(&self, _lc: &crate::i18n::LcRegistry) -> String {
        "Voz de Nexum — F1 mock (status | test | privacy | engines | voices | set-voice | style | reset-voice | listen | stop)".to_string()
    }

    fn execute(&self, app: &mut App, args: &str) {
        match args.trim().split_whitespace().next().unwrap_or("status") {
            "status" | "" => cmd_status(app),
            "test" => cmd_test(app),
            "engines" => {
                let lines = crate::voice::headless::engines_summary().join("\n⎿ ");
                app.push_system_note(format!("Voice engines (F2)\n⎿ {lines}"));
            }
            "listen" | "toggle" => {
                // La captura corre headless (cliente del runtime, no de la
                // TUI): se delega al propio binario en modo voice.
                let exe = std::env::current_exe()
                    .unwrap_or_else(|_| std::path::PathBuf::from("nexum"));
                match std::process::Command::new(exe)
                    .args(["voice", "--toggle"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(_) => app.push_system_note(
                        "Voz: escucha alternada (toggle). Overlay/notificación \
                         indica el estado; repetí /voz listen o Super+Z para cortar."
                            .to_string(),
                    ),
                    Err(e) => app.push_system_note(format!("Voz: no pude lanzar la escucha: {e}")),
                }
            }
            "style" => {
                let arg = args.trim().split_whitespace().nth(1).unwrap_or("");
                match crate::voice::profile::VoiceStyleController::apply_style(arg) {
                    Ok(p) => app.push_system_note(format!("Voz: estilo {} ({}) · speed {:.2}", p.display_name, p.tone, p.speed)),
                    Err(e) => app.push_system_note(format!("Voz: {e}")),
                }
            }
            "reset-voice" => {
                let _ = crate::voice::profile::VoiceStyleController::apply_style("reset");
                app.push_system_note("Voz: perfil default de Nexum restaurado.".to_string());
            }
            "voices" => {
                let active = crate::voice::profile::load();
                let vs = crate::voice::profile::list_voices();
                let list = vs.iter().map(|v| format!("{}{}", if *v == active.voice_id {"* "} else {"  "}, v)).collect::<Vec<_>>().join("\n⎿ ");
                app.push_system_note(format!("Perfil: {} · estilo {}\n⎿ {list}", active.display_name, active.tone));
            }
            "set-voice" => {
                let id = args.trim().split_whitespace().nth(1).unwrap_or("");
                if crate::voice::profile::list_voices().iter().any(|v| v == id) {
                    let mut p = crate::voice::profile::load();
                    p.voice_id = id.to_string();
                    let _ = crate::voice::profile::save(&p);
                    app.push_system_note(format!("Voz activa: {id}"));
                } else {
                    app.push_system_note(format!("Voz '{id}' no encontrada — /voz voices"));
                }
            }
            "stop" => {
                let exe = std::env::current_exe()
                    .unwrap_or_else(|_| std::path::PathBuf::from("nexum"));
                let _ = std::process::Command::new(exe).args(["voice", "--stop"]).output();
                app.push_system_note("Voz: sesión de escucha cancelada (audio descartado).".to_string());
            }
            "privacy" => {
                let lines = VoicePrivacyPolicy::status_lines().join("\n⎿ ");
                app.push_system_note(format!("Privacidad de voz (F1)\n⎿ {lines}"));
            }
            other => app.push_system_note(format!(
                "/voz: subcomando desconocido '{other}'. Usá: status | test | privacy"
            )),
        }
        app.render_rebuild();
    }
}

fn cmd_status(app: &mut App) {
    let horm = crate::hormiguero::bridge().status();
    let horm_txt = if horm.enabled && horm.sidecar_alive {
        "disponible"
    } else {
        "no disponible (la voz local necesita el pasillo Hormiguero)"
    };
    app.push_system_note(format!(
        "Voz de Nexum (F1 — mock)\n\
         ⎿ voz habilitada: {} (flag NEXUM_VOICE; gobierna la futura captura real)\n\
         ⎿ modo: mock/headless — sin micrófono\n\
         ⎿ ASR real: no configurado (F2: whisper.cpp; F3: Parakeet tras benchmark)\n\
         ⎿ TTS real: no configurado (F2: Piper opcional)\n\
         ⎿ hotkey: {} (configurado, NO registrado en F1)\n\
         ⎿ Hormiguero: {horm_txt}\n\
         ⎿ privacidad: local/mock, sin audio cloud, sin grabación",
        if voice::voice_enabled() { "on" } else { "off" },
        VoiceHotkeyManager::configured_hotkey(),
    ));
}

fn cmd_test(app: &mut App) {
    let mut asr = MockAsrAdapter;
    let mut tts = MockTtsAdapter;
    let mut overlay = LogOverlay::default();
    let mut out = String::from("/voz test — pipeline mock (allow_paid_ai=false)\n");

    for (label, fixture) in [
        ("trivial", "hola Nexum, estás?"),
        ("compleja", "Analizá esta arquitectura y proponé mejoras"),
    ] {
        overlay.show(OverlayState::Listening);
        let transcript = asr.transcribe_fixture(fixture);
        overlay.show(OverlayState::Transcribing);
        let start = std::time::Instant::now();
        // Garantía dura F1: allow_paid_ai=false ⇒ 0 llamadas pagas.
        let resp = VoiceIntentRouter::route(&transcript, "es", false);
        let ms = start.elapsed().as_millis();
        overlay.show(match resp.handled_by {
            HandledBy::Local => OverlayState::LocalHandling,
            HandledBy::PaidAi => OverlayState::Escalating,
            HandledBy::Blocked => OverlayState::PrivacyBlocked,
        });
        let spoken = if resp.should_speak {
            overlay.show(OverlayState::Speaking);
            tts.speak(&resp.text_speakable)
        } else {
            None
        };
        out.push_str(&format!(
            "⎿ [{label}] \"{fixture}\" ({ms}ms)\n\
             ⎿   handled_by: {} · provider pago: NO llamado\n\
             ⎿   speakable: {}\n",
            resp.handled_by,
            spoken.unwrap_or_else(|| "(no habla)".to_string()),
        ));
    }
    let states: Vec<String> = overlay.seen.iter().map(|s| format!("{s:?}")).collect();
    out.push_str(&format!("⎿ overlay simulado: {}", states.join(" → ")));
    app.push_system_note(out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hormiguero::bridge::test_env_lock;

    #[tokio::test]
#[allow(clippy::await_holding_lock)] // test serializes env mutation via a std Mutex; keeping the guard held across await is intentional
    async fn test_voz_status_renderiza_sin_crash_y_sin_secretos() {
        let _g = test_env_lock();
        std::env::remove_var("NEXUM_VOICE");
        std::env::remove_var("NEXUM_HORMIGUERO");
        let (mut app, _) = crate::app::App::new_headless(80, 24).await;
        VozCommand.execute(&mut app, "status");
        let last = app
            .session_mgr
            .current()
            .messages
            .view_messages
            .last()
            .cloned();
        let text = match last {
            Some(crate::ui::message_view::MessageViewModel::SystemNote { content, .. }) => content,
            other => panic!("esperaba SystemNote: {other:?}"),
        };
        assert!(text.contains("voz habilitada: off"));
        assert!(text.contains("sin audio cloud"));
        assert!(!text.contains("qwen"), "sin modelos internos: {text}");
    }

    #[tokio::test]
#[allow(clippy::await_holding_lock)] // test serializes env mutation via a std Mutex; keeping the guard held across await is intentional
    async fn test_voz_test_no_crashea_con_sidecar_muerto() {
        let _g = test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("NEXUM_HORMIGUERO_RUNTIME_DIR", dir.path());
        std::env::set_var("NEXUM_HORMIGUERO", "on");
        crate::hormiguero::bridge().reset_for_test();
        let (mut app, _) = crate::app::App::new_headless(80, 24).await;
        VozCommand.execute(&mut app, "test");
        std::env::remove_var("NEXUM_HORMIGUERO_RUNTIME_DIR");
        std::env::remove_var("NEXUM_HORMIGUERO");
        let n = app.session_mgr.current().messages.view_messages.len();
        assert!(n >= 1, "deja nota degradada, TUI viva");
    }

    #[tokio::test]
#[allow(clippy::await_holding_lock)] // test serializes env mutation via a std Mutex; keeping the guard held across await is intentional
    async fn test_voz_privacy_muestra_politica() {
        let _g = test_env_lock();
        let (mut app, _) = crate::app::App::new_headless(80, 24).await;
        VozCommand.execute(&mut app, "privacy");
        let last = app
            .session_mgr
            .current()
            .messages
            .view_messages
            .last()
            .cloned();
        if let Some(crate::ui::message_view::MessageViewModel::SystemNote { content, .. }) = last {
            assert!(content.contains("audio a cloud: NUNCA"));
        } else {
            panic!("esperaba SystemNote");
        }
    }
}
