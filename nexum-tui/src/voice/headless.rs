//! Voice F2 — controller HEADLESS: `nexum voice --status|--test|--listen|
//! --toggle|--stop`. Cliente del runtime (sidecar Hormiguero + pipeline
//! F1); cero dependencia de la TUI, cero xdotool, cero cloud.
//!
//! Sesión: pidfile `voice.pid` en el runtime dir. `--toggle` = si hay
//! sesión escuchando manda SIGUSR1 (cortar y procesar); si no, lanza una
//! detached (para el atajo KDE Super+Z). `--stop` = SIGTERM (cancela y
//! descarta audio). Timeout duro de 60s por escucha.

use super::adapters::{OverlayState, VoiceOverlay};
use super::audio::{discard_wav, Recorder};
use super::hud_model::HudPhase;
use super::intent_router::VoiceIntentRouter;
use super::overlay_notify::NotificationOverlay;
use super::{asr_whisper, tts_piper, HandledBy, HudHint};
use std::path::PathBuf;

const PID_FILE: &str = "voice.pid";

/// Directorio del pid file del daemon de voz.
///
/// El fallback iba a `std::env::temp_dir()` PELADO, o sea `/tmp/voice.pid` con
/// nombre fijo. En una máquina con más de un usuario eso es un pid file
/// compartido: `read_live_pid` lee el PID de otro, verifica que el cmdline
/// contenga "voice", y lo da por propio — con `stop` señalando el proceso
/// ajeno.
///
/// Acá el PID NO corresponde, y es el contraejemplo útil: este archivo existe
/// justamente para que OTRO proceso lo encuentre. Es estado compartido por
/// USUARIO, así que el aislamiento va por UID, no por proceso.
pub(super) fn runtime_dir() -> PathBuf {
    crate::hormiguero::bridge::runtime_dir().unwrap_or_else(|| {
        #[cfg(unix)]
        // SAFETY: getuid() no falla y no tiene efectos.
        let uid = unsafe { libc::getuid() };
        #[cfg(not(unix))]
        let uid = 0;
        #[allow(clippy::disallowed_methods)] // fallback de último recurso, aislado por UID
        let dir = std::env::temp_dir().join(format!("nexum-voice-{uid}"));
        let _ = std::fs::create_dir_all(&dir);
        dir
    })
}

fn read_live_pid(dir: &std::path::Path) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(dir.join(PID_FILE)).ok()?.trim().parse().ok()?;
    // ¿vivo y es nuestro comando voice?
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
    (cmdline.contains("voice")).then_some(pid)
}

#[cfg(unix)]
unsafe fn send_sig(pid: i32, sig: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid, sig) };
}

#[cfg(not(unix))]
unsafe fn send_sig(_pid: i32, _sig: i32) {}

#[cfg(not(unix))]
struct NoSignals;

#[cfg(not(unix))]
impl NoSignals {
    fn recv(&mut self) -> futures_util::future::Pending<()> {
        futures_util::future::pending()
    }
}

// ── status / engines ─────────────────────────────────────────────────────

pub fn engines_summary() -> Vec<String> {
    let (asr, asr_d) = asr_whisper::detect();
    let (_tts, tts_d) = tts_piper::detect();
    let rec = super::audio::detect_backend().unwrap_or("NO disponible");
    vec![
        format!("grabación: {rec}"),
        format!("ASR: {asr_d} → modo {}", if asr.is_some() { "local" } else { "mock/test" }),
        format!("TTS: {tts_d}"),
        "audio cloud: NUNCA (no existe adapter)".to_string(),
    ]
}

pub fn run_status() -> i32 {
    let horm = crate::hormiguero::bridge().status();
    println!("Nexum Voice (F2)");
    for l in engines_summary() {
        println!("  {l}");
    }
    println!(
        "  Hormiguero: {}",
        if horm.enabled && horm.sidecar_alive { "disponible" } else { "no disponible (voz local limitada)" }
    );
    let engines = super::hud_model::EnginesInfo::detect();
    println!(
        "  modelo configurado: {}",
        engines
            .configured
            .map(|m| format!("{} — {}", m.label, m.kind))
            .unwrap_or_else(|| "no configurado (decí «Nexum, cambiá a …» o usá la TUI)".into())
    );
    println!("  hotkey recomendado: {} → nexum voice --toggle (ver docs/user/NEXUM_VOICE_HOTKEY_KDE_SETUP.md)",
        std::env::var("NEXUM_VOICE_HOTKEY").unwrap_or_else(|_| "Super+Z".into()));
    let dir = runtime_dir();
    println!(
        "  sesión de escucha: {}",
        if read_live_pid(&dir).is_some() { "ACTIVA" } else { "ninguna" }
    );
    println!("  privacidad: push-to-talk manual, sin always-listening, sin guardar audio");
    0
}

pub fn run_test() -> i32 {
    // Mismo pipeline mock F1 (sin micrófono), imprime a stdout.
    println!("nexum voice --test (mock, allow_paid_ai=false)");
    for (label, fixture) in [
        ("trivial", "hola Nexum, estás?"),
        ("compleja", "Analizá esta arquitectura y proponé mejoras"),
    ] {
        let r = VoiceIntentRouter::route(fixture, "es", false);
        println!("  [{label}] handled_by={} · provider pago: NO llamado", r.handled_by);
        println!("    speakable: {}", r.text_speakable);
    }
    0
}

// ── toggle / stop ────────────────────────────────────────────────────────

pub fn run_toggle() -> i32 {
    let dir = runtime_dir();
    if let Some(pid) = read_live_pid(&dir) {
        unsafe { send_sig(pid, 10) }; // SIGUSR1: cortar y procesar
        return 0;
    }
    // Sin sesión: lanzar una escucha one-shot detached (para el atajo KDE,
    // que no tiene TTY). stdout/err van al log del runtime dir.
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("nexum"));
    let log = std::fs::File::create(dir.join("voice.log")).ok();
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["voice", "--listen"]).stdin(std::process::Stdio::null());
    if let Some(l) = log {
        let l2 = l.try_clone().ok();
        cmd.stdout(l);
        if let Some(l2) = l2 {
            cmd.stderr(l2);
        }
    }
    match cmd.spawn() {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("[voice] no pude lanzar la escucha: {e}");
            1
        }
    }
}

pub fn run_stop() -> i32 {
    let dir = runtime_dir();
    if let Some(pid) = read_live_pid(&dir) {
        unsafe { send_sig(pid, 15) }; // SIGTERM: cancelar y descartar
        println!("[voice] sesión cancelada (audio descartado)");
    } else {
        println!("[voice] no hay sesión de escucha activa");
    }
    0
}

/// Habla con el engine del perfil activo (kokoro|piper) con fallback Piper.
pub fn speak_with_active_profile(text: &str, dir: &std::path::Path) {
    let p = super::profile::load();
    if p.engine == "kokoro" {
        if let (Some(k), _) = super::tts_kokoro::detect() {
            if k.speak(text, &p.voice_id, p.speed, dir).is_ok() {
                return;
            }
        }
        eprintln!("[voice] kokoro no disponible → fallback Piper");
    }
    match tts_piper::detect() {
        (Some(t), _) => {
            if let Err(e) = t.speak(text, dir) {
                // NUNCA en silencio. El error descartado acá es la razón por la
                // que `--preview` podía imprimir tres perfiles sin que sonara
                // ninguno, y por la que desde afuera no había forma de saber si
                // la voz andaba.
                eprintln!("[voice] la síntesis falló: {e}");
            }
        }
        (None, motivo) => eprintln!("[voice] sin TTS: {motivo}"),
    }
}

pub fn run_preview(target: &str) -> i32 {
    let dir = runtime_dir();
    let cur = super::profile::load();
    let frase = "Hola Nico, soy Nexum. Esta es una muestra de mi voz.";
    let cands: Vec<_> = if target.is_empty() {
        super::catalog::preview_candidates(&cur.id)
    } else {
        super::catalog::catalog().into_iter().filter(|e| e.id == target).collect()
    };
    if cands.is_empty() {
        eprintln!("[voice] perfil '{target}' no encontrado (mirá --voices)");
        return 1;
    }
    let mut fallos = 0;
    for e in cands.iter().take(3) {
        println!("[voice] preview: {} ({} · {}Hz)", e.display_name, e.engine, e.perceived_pitch_hz);
        let r = if e.engine == "kokoro" {
            match super::tts_kokoro::detect() {
                (Some(k), _) => k.speak(frase, e.engine_voice_id, 1.0, &dir),
                (None, motivo) => Err(motivo),
            }
        } else {
            match tts_piper::detect() {
                (Some(t), _) => t.speak(frase, &dir),
                (None, motivo) => Err(motivo),
            }
        };
        if let Err(err) = r {
            // Un preview que imprime el perfil sin haber sonado es peor que uno
            // que no imprime nada: deja creyendo que la voz anda.
            eprintln!("[voice]   ✗ NO sonó: {err}");
            fallos += 1;
        }
    }
    if fallos > 0 { 1 } else { 0 }
}

pub fn run_previous_voice() -> i32 {
    match super::profile::restore_previous() {
        Ok(p) => { println!("[voice] voz restaurada: {}", p.display_name); 0 }
        Err(e) => { eprintln!("[voice] {e}"); 1 }
    }
}

pub fn run_hud_test() -> i32 {
    use super::hud::VoiceHud;
    let dir = runtime_dir();
    let mut hud = VoiceHud::spawn(&dir);
    println!("[voice] HUD test — backend: {}", if hud.has_window() { "ventana QML" } else { "notificación/stderr" });
    // Los 19 estados del modelo (demo visual completa).
    for ph in [
        HudPhase::Idle, HudPhase::PreparingRuntime, HudPhase::ReconnectingRuntime,
        HudPhase::RuntimeUnavailable, HudPhase::Listening, HudPhase::SpeechDetected,
        HudPhase::SilenceCountdown, HudPhase::NoSpeech, HudPhase::Transcribing,
        HudPhase::LocalHandling, HudPhase::Escalating, HudPhase::Speaking,
        HudPhase::Success, HudPhase::Error, HudPhase::Fallback,
        HudPhase::VoiceChanged, HudPhase::ModelChanged,
        HudPhase::WaitingConfirmation, HudPhase::PrivacyBlocked,
    ] {
        hud.show_phase(ph, Some("(demo)"));
        std::thread::sleep(std::time::Duration::from_millis(900));
    }
    hud.show_answer("HUD de Nexum operativo. (test, sin micrófono)", "local");
    hud.close();
    0
}

/// Turno por TEXTO (misma ruta que la voz, sin micrófono ni TTS).
/// Sirve para probar directivas ("mostrame el modelo actual") y para
/// accesibilidad/scripts. 0 audio, 0 cloud.
pub fn run_say(frase: &str) -> i32 {
    // Fast path de acción local (RC-2): crear carpeta, HITL de dos pasos por
    // texto (la confirmación es otro `--say "sí"`). 0 provider, 0 tokens.
    let dir = runtime_dir();
    let workspace = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".into());
    match super::super::local_action::dispatch(frase, std::path::Path::new(&workspace), &dir) {
        super::super::local_action::FastPathOutcome::NotLocal => {}
        super::super::local_action::FastPathOutcome::Executed(m)
        | super::super::local_action::FastPathOutcome::Cancelled(m)
        | super::super::local_action::FastPathOutcome::Proposed(m)
        | super::super::local_action::FastPathOutcome::Rejected(m) => {
            println!("[voice] local_action → {m}");
            return 0;
        }
    }
    let decision = VoiceIntentRouter::route_decision(frase, "es");
    if matches!(
        &decision,
        super::acp_turn::VoiceRouteDecision::Escalate { .. }
            | super::acp_turn::VoiceRouteDecision::ModelDirective { .. }
    ) {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("[voice] no pude iniciar el cliente ACP: {error}");
                return 1;
            }
        };
        let store = super::acp_turn::VoiceSessionStore::at_path(
            runtime_dir().join("voice-acp-session.json"),
        );
        let result = runtime.block_on(async {
            let client = super::acp_turn::VoiceRuntimeBootstrap::connect_default().await?;
            super::acp_turn::VoiceTurnController::with_preferences_store(
                client,
                store,
                super::acp_turn::VoiceHudBridge::default(),
                super::acp_turn::VoicePreferencesStore::at_path(
                    super::acp_turn::VoicePreferencesStore::default_path(),
                ),
            )
            .execute(decision, ".")
            .await
        });
        return match result {
            Ok(result) => {
                println!("[voice] escalated → {}", result.speakable);
                0
            }
            Err(error) => {
                eprintln!("[voice] no pude completar el pedido: {error}");
                1
            }
        };
    }
    let r = VoiceIntentRouter::route(frase, "es", false);
    println!("[voice] {} → {}", r.handled_by, r.text_full);
    if r.text_speakable != r.text_full {
        println!("[voice] speakable: {}", r.text_speakable);
    }
    println!("[voice] hud_hint: {:?}", r.hud_hint);
    0
}

/// Entrada headless inyectable para tests e integraciones: ejercita la misma
/// decisión Hormiguero -> ACP que usará `--say` cuando haya runtime local.
/// No crea proveedores, herramientas ni ciclos de agente propios.
pub async fn run_say_with_acp(
    frase: &str,
    client: super::acp_turn::VoiceAcpClient,
    store: super::acp_turn::VoiceSessionStore,
    workspace: &str,
) -> Result<super::acp_turn::VoiceTurnResult, super::acp_turn::VoiceTurnError> {
    let decision = VoiceIntentRouter::route_decision(frase, "es");
    super::acp_turn::VoiceTurnController::with_preferences_store(
        client,
        store,
        super::acp_turn::VoiceHudBridge::default(),
        super::acp_turn::VoicePreferencesStore::at_path(
            super::acp_turn::VoicePreferencesStore::default_path(),
        ),
    )
    .execute(decision, workspace)
    .await
}

pub fn run_style(style: &str) -> i32 {
    // Ids exactos O frases naturales ("más grave y tranquilo").
    if style.split_whitespace().count() > 1 {
        return match super::profile::VoiceStyleController::apply_natural(style) {
            Ok((p, msg)) => { println!("[voice] {msg} [{}]", p.id); 0 }
            Err(e) => { eprintln!("[voice] {e}"); 1 }
        };
    }
    match super::profile::VoiceStyleController::apply_style(style) {
        Ok(p) => { println!("[voice] estilo: {} ({}) · speed {:.2}", p.display_name, p.tone, p.speed); 0 }
        Err(e) => { eprintln!("[voice] {e}"); 1 }
    }
}

pub fn run_speed(v: f32) -> i32 {
    match super::profile::VoiceStyleController::set_speed(v) {
        Ok(p) => { println!("[voice] speed: {:.2}", p.speed); 0 }
        Err(e) => { eprintln!("[voice] {e}"); 1 }
    }
}

pub fn run_pitch(v: f32) -> i32 {
    match super::profile::VoiceStyleController::set_pitch(v) {
        Ok(p) => { println!("[voice] pitch: {:+.1} (piper no lo aplica aún; queda en el perfil para engines F3)", p.pitch); 0 }
        Err(e) => { eprintln!("[voice] {e}"); 1 }
    }
}

/// Dice el costo MEDIDO de elegir una voz de rol narración.
///
/// No cambia la elección: pisar una decisión explícita del usuario es de
/// producto, no de esta capa. Pero elegir una voz que tarda 3.7 s en contestar
/// "listo" no puede pasar en silencio — el número existe, es medido, y
/// escondérselo al que elige es la misma clase de mentira que un doctor que
/// dice LISTO sin sintetizar.
fn advertir_si_es_narrador(voice_id: &str) {
    use super::catalog::Rol;
    let Some(e) = super::catalog::catalog()
        .into_iter()
        .find(|e| e.engine_voice_id == voice_id || e.id == voice_id)
    else {
        return;
    };
    if e.rol == Rol::Narracion {
        eprintln!(
            "[voice] ojo: {} es una voz de NARRACIÓN — {} ms hasta el primer audio \
             (medido). Sirve para texto largo; para confirmaciones va a sentirse lenta.",
            e.display_name, e.first_audio_ms_short
        );
    }
}

pub fn run_voices() -> i32 {
    let active = super::profile::load();
    println!("Perfil: {} [{}] · voz {} · engine {} · speed {:.2}", active.display_name, active.id, active.voice_id, active.engine, active.speed);
    println!("Voces instaladas:");
    let voices = super::profile::list_voices();
    if voices.is_empty() {
        println!("  (ninguna) — instalá voces Piper .onnx en ~/.local/share/piper/voices/");
    }
    for v in voices {
        println!("  {}{}", if v == active.voice_id { "* " } else { "  " }, v);
    }
    0
}

pub fn run_set_voice(id: &str) -> i32 {
    let mut p = super::profile::load();
    if !super::profile::list_voices().iter().any(|v| v == id) {
        eprintln!("[voice] voz '{id}' no encontrada — mirá `nexum voice --voices`");
        return 1;
    }
    p.voice_id = id.to_string();
    match super::profile::save(&p) {
        Ok(()) => {
            println!("[voice] voz activa: {id}");
            advertir_si_es_narrador(id);
            0
        }
        Err(e) => { eprintln!("[voice] no pude guardar: {e}"); 1 }
    }
}

// ── listen (el flujo real) ───────────────────────────────────────────────

pub async fn run_listen() -> i32 {
    // Public demo: NUNCA voz real (privacidad).
    if crate::ui::demo_mode::public_demo_enabled() {
        let mut ov = NotificationOverlay::new();
        ov.show(OverlayState::PrivacyBlocked);
        eprintln!("[voice] public demo activo: voz real deshabilitada");
        return 2;
    }
    let dir = runtime_dir();
    if read_live_pid(&dir).is_some() {
        eprintln!("[voice] ya hay una sesión de escucha activa");
        return 1;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(PID_FILE), std::process::id().to_string());
    let code = listen_inner(&dir).await;
    let _ = std::fs::remove_file(dir.join(PID_FILE));
    code
}

async fn listen_inner(dir: &std::path::Path) -> i32 {
    use super::hud::VoiceHud;
    use super::vad::{RmsVad, VadConfig, VadVerdict};
    let mut hud = VoiceHud::spawn(dir);
    // Secuencia F2.2.1: HUD visible → asegurar runtime → recién ahí el mic.
    hud.show(OverlayState::PreparingRuntime);
    if let Err(e) = super::bootstrap::ensure_local_runtime() {
        hud.show(OverlayState::RuntimeUnavailable);
        eprintln!("[voice] {e}");
        std::thread::sleep(std::time::Duration::from_secs(10));
        hud.close();
        return 1;
    }
    hud.show(OverlayState::Listening);
    let recorder = match Recorder::start(dir) {
        Ok(r) => r,
        Err(e) => {
            hud.show(OverlayState::Error);
            eprintln!("[voice] {e}");
            hud.close();
            return 1;
        }
    };

    // Loop VAD: corta solo por silencio tras voz; señales siguen andando
    // (SIGUSR1 = cortar ya; SIGTERM/Ctrl+C = cancelar; max = timeout duro).
    #[cfg(unix)]
    let (mut usr1, mut term) = {
        use tokio::signal::unix::{signal, SignalKind};
        (
            signal(SignalKind::user_defined1()).expect("signal"),
            signal(SignalKind::terminate()).expect("signal"),
        )
    };
    #[cfg(not(unix))]
    let (mut usr1, mut term) = (NoSignals, NoSignals);
    let cfg = VadConfig::from_env();
    let max_secs = cfg.max_record_secs;
    let mut vad = RmsVad::new(cfg);
    let started = std::time::Instant::now();
    let mut last_state = OverlayState::Listening;
    let outcome = loop {
        tokio::select! {
            _ = usr1.recv() => break "cut",
            _ = term.recv() => break "cancel",
            _ = tokio::signal::ctrl_c() => break "cancel",
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                if started.elapsed().as_secs() >= max_secs { break "cut"; }
                let v = vad.poll(&recorder.wav_path, 0.25);
                let st = match v {
                    VadVerdict::Speech => OverlayState::SpeechDetected,
                    VadVerdict::SilenceCountdown => OverlayState::SilenceCountdown,
                    VadVerdict::CutSilence => break "cut",
                    VadVerdict::CutNoSpeech => break "nospeech",
                    VadVerdict::WaitingSpeech => OverlayState::Listening,
                };
                if st != last_state { hud.show(st); last_state = st; }
            }
        }
    };

    match outcome {
        "cancel" => {
            recorder.cancel();
            hud.show(OverlayState::PrivacyBlocked);
            eprintln!("[voice] cancelado; audio descartado");
            hud.close();
            return 0;
        }
        "nospeech" => {
            recorder.cancel();
            hud.show_phase(HudPhase::NoSpeech, None);
            if !hud.has_window() {
                hud.show_answer("No escuché nada claro. Probá de nuevo más cerca del micrófono.", "local");
            }
            hud.close();
            return 0;
        }
        _ => {}
    }

    hud.show(OverlayState::Transcribing);
    let wav = match recorder.stop() {
        Ok(w) => w,
        Err(e) => {
            hud.show(OverlayState::Error);
            eprintln!("[voice] {e}");
            hud.close();
            return 1;
        }
    };

    let (asr, asr_detail) = asr_whisper::detect();
    let transcript = match asr {
        Some(w) => {
            let r = w.transcribe(&wav, "es");
            discard_wav(&wav); // SIEMPRE: el audio no se persiste.
            match r {
                Ok(t) => t,
                Err(e) => {
                    hud.show(OverlayState::Error);
                    eprintln!("[voice] ASR: {e}");
                    hud.close();
                    return 1;
                }
            }
        }
        None => {
            discard_wav(&wav);
            hud.show(OverlayState::Error);
            eprintln!("[voice] {asr_detail}");
            hud.close();
            return 2;
        }
    };

    // ── 1. Fast path de acción local (RC-2 / NCP-LOCAL-ACTION): crear carpeta.
    //    Determinístico, 0 provider, 0 tokens, HITL por voz de dos pasos.
    let workspace = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".into());
    match super::super::local_action::dispatch(&transcript, std::path::Path::new(&workspace), dir) {
        super::super::local_action::FastPathOutcome::NotLocal => { /* sigue al runtime */ }
        outcome => {
            let (msg, phase) = match outcome {
                super::super::local_action::FastPathOutcome::Executed(m) => (m, HudPhase::VoiceChanged),
                super::super::local_action::FastPathOutcome::Cancelled(m) => (m, HudPhase::Fallback),
                super::super::local_action::FastPathOutcome::Proposed(m) => (m, HudPhase::WaitingConfirmation),
                super::super::local_action::FastPathOutcome::Rejected(m) => (m, HudPhase::Fallback),
                super::super::local_action::FastPathOutcome::NotLocal => unreachable!(),
            };
            let speak = super::make_speakable(&msg);
            hud.show(OverlayState::LocalHandling);
            println!("[voice] local_action → {msg}");
            hud.show(OverlayState::Speaking);
            speak_with_active_profile(&speak, dir);
            hud.show_phase(phase, Some(&speak));
            hud.close();
            return 0;
        }
    }

    // ── 2. Cambio de voz/estilo local (0 tokens, 0 provider).
    let local_directive = VoiceIntentRouter::route(&transcript, "es", false);
    if matches!(local_directive.handled_by, HandledBy::Local)
        && local_directive.hud_hint == HudHint::VoiceChanged
    {
        hud.show(OverlayState::Speaking);
        speak_with_active_profile(&local_directive.text_speakable, dir);
        hud.show_phase(HudPhase::VoiceChanged, Some(&local_directive.text_speakable));
        hud.close();
        return 0;
    }

    // ── 3. Delegación al runtime (RC-2 fix P1-VOICE-DELEGATION): toda tarea no
    //    trivial va al MISMO runtime/provider que una consulta escrita. Voice
    //    NO bloquea con "requiere modelo avanzado": el runtime decide con el
    //    provider que el usuario ya seleccionó.
    hud.show(OverlayState::Escalating);
    match delegate_to_runtime(&transcript, dir).await {
        Ok(result) => {
            println!("[voice] runtime → {}", result.speakable);
            hud.show(OverlayState::Speaking);
            speak_with_active_profile(&result.speakable, dir);
            hud.show_answer(&result.speakable, "runtime");
            hud.close();
            0
        }
        Err(e) => {
            // Error accionable, no negativa genérica.
            let msg = format!(
                "No pude completar el pedido con el proveedor configurado: {e}. \
                 Configurá o revisá el proveedor con /proveedor en Nexum."
            );
            hud.show(OverlayState::RuntimeUnavailable);
            eprintln!("[voice] {msg}");
            speak_with_active_profile("No pude completar el pedido con el proveedor configurado.", dir);
            std::thread::sleep(std::time::Duration::from_secs(2));
            hud.close();
            1
        }
    }
}

/// Delega un transcript al runtime local vía ACP: la misma ruta que una
/// consulta escrita. Usa el provider/modelo seleccionado por el usuario.
/// Voice NO cambia el provider ni autoriza uno distinto (SPEC-VOICE-001).
async fn delegate_to_runtime(
    transcript: &str,
    dir: &std::path::Path,
) -> Result<super::acp_turn::VoiceTurnResult, super::acp_turn::VoiceTurnError> {
    let decision = VoiceIntentRouter::route_decision(transcript, "es");
    let store =
        super::acp_turn::VoiceSessionStore::at_path(dir.join("voice-acp-session.json"));
    let client = super::acp_turn::VoiceRuntimeBootstrap::connect_default().await?;
    super::acp_turn::VoiceTurnController::with_preferences_store(
        client,
        store,
        super::acp_turn::VoiceHudBridge::default(),
        super::acp_turn::VoicePreferencesStore::at_path(
            super::acp_turn::VoicePreferencesStore::default_path(),
        ),
    )
    .execute(decision, ".")
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hormiguero::bridge::test_env_lock;

    #[test]
    fn test_engines_summary_reporta_sin_crash_y_sin_cloud() {
        let s = engines_summary().join("\n");
        assert!(s.contains("audio cloud: NUNCA"));
        assert!(s.contains("ASR:"), "reporta estado de whisper: {s}");
    }

    #[test]
    fn test_run_status_y_test_exit_cero_sin_microfono() {
        let _g = test_env_lock();
        std::env::remove_var("NEXUM_HORMIGUERO");
        assert_eq!(run_status(), 0);
        assert_eq!(run_test(), 0, "--test funciona sin micrófono real");
    }

    #[tokio::test]
#[allow(clippy::await_holding_lock)] // test serializes env mutation via a std Mutex; keeping the guard held across await is intentional
    async fn test_public_demo_bloquea_escucha_real() {
        let _g = test_env_lock();
        std::env::set_var("NEXUM_PUBLIC_DEMO", "1");
        let code = run_listen().await;
        std::env::remove_var("NEXUM_PUBLIC_DEMO");
        assert_eq!(code, 2, "public demo jamás abre el micrófono");
    }

    #[test]
    fn test_whisper_ausente_degrada_con_mensaje() {
        let _g = test_env_lock();
        // Forzar rutas inexistentes para simular ausencia aunque estuviera.
        std::env::set_var("NEXUM_WHISPER_BIN", "/nonexistent/whisper");
        std::env::set_var("PATH", "/nonexistent");
        let (asr, detail) = asr_whisper::detect();
        std::env::remove_var("NEXUM_WHISPER_BIN");
        assert!(asr.is_none());
        assert!(detail.contains("NO instalado"), "mensaje claro: {detail}");
    }
}

#[cfg(test)]
mod rc2_tests {
    use super::*;
    use crate::hormiguero::bridge::test_env_lock;

    #[test]
    fn test_fast_path_local_no_escala_ni_bloquea() {
        let _g = test_env_lock();
        std::env::remove_var("NEXUM_HORMIGUERO");
        #[allow(clippy::disallowed_methods)] // lleva PID
        let ws = std::env::temp_dir().join(format!("rc2-ws-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let state = ws.join("st");
        std::fs::create_dir_all(&state).unwrap();
        // "creá una carpeta" NO debe ser NotLocal (no llega al router que
        // bloquearía con "requiere modelo avanzado").
        let out = crate::local_action::dispatch(
            "creá una carpeta llamada Rc2Prueba en el proyecto actual",
            &ws,
            &state,
        );
        assert!(matches!(out, crate::local_action::FastPathOutcome::Proposed(_)),
            "acción local propone HITL, no escala a modelo pago");
        std::fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn test_conversacion_normal_es_notlocal_para_delegar() {
        let _g = test_env_lock();
        #[allow(clippy::disallowed_methods)] // lleva PID
        let ws = std::env::temp_dir().join(format!("rc2-ws2-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let state = ws.join("st");
        std::fs::create_dir_all(&state).unwrap();
        // Una consulta de razonamiento cae a NotLocal → se delega al runtime
        // (no al canned refusal).
        let out = crate::local_action::dispatch("analizá esta arquitectura y proponé mejoras", &ws, &state);
        assert!(matches!(out, crate::local_action::FastPathOutcome::NotLocal),
            "tarea no trivial se delega al runtime, no se maneja localmente");
        std::fs::remove_dir_all(&ws).ok();
    }
}
