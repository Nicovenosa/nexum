//! `nexum voice --doctor` (F2.2): diagnóstico completo del stack de voz,
//! y `--install-shortcut-kde`: registro seguro/reversible del atajo
//! global (Super+Z) vía config de KDE — sin xdotool, sin hacks.

use std::path::PathBuf;
use std::process::Command;

pub const HOTKEY_DEFAULT: &str = "Meta+Z"; // Super+Z (decisión de Nico)

fn hotkey() -> String {
    std::env::var("NEXUM_VOICE_HOTKEY").unwrap_or_else(|_| HOTKEY_DEFAULT.to_string())
}

fn ok(b: bool) -> &'static str {
    if b { "✓" } else { "✗" }
}

fn in_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

fn shortcuts_rc() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_default()
        .join(".config/kglobalshortcutsrc")
}

/// ¿El rc tiene nexum-voice.desktop con el hotkey? (configurado)
fn shortcut_in_rc(key: &str) -> bool {
    std::fs::read_to_string(shortcuts_rc())
        .map(|s| {
            s.split("[services]")
                .any(|sec| sec.contains("nexum-voice.desktop") && sec.contains(&format!("_launch={key}")))
                || s.contains("nexum-voice.desktop]")
                    && s.contains(&format!("_launch={key}"))
        })
        .unwrap_or(false)
}

/// ¿kglobalaccel (vivo) ya cargó el componente? (activo)
fn shortcut_active_in_daemon() -> bool {
    Command::new("qdbus6")
        .args([
            "--literal",
            "org.kde.kglobalaccel",
            "/kglobalaccel",
            "org.kde.KGlobalAccel.allComponents",
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("nexum"))
        .unwrap_or(false)
}

/// ¿Quién más tiene el hotkey en el rc? (conflicto)
fn hotkey_conflict(key: &str) -> Option<String> {
    let s = std::fs::read_to_string(shortcuts_rc()).ok()?;
    let mut current = String::new();
    for line in s.lines() {
        if line.starts_with('[') {
            current = line.to_string();
        } else if line.contains(&format!("={key}")) && !current.contains("nexum-voice") {
            return Some(current.clone());
        }
    }
    None
}

pub fn run_doctor() -> i32 {
    println!("nexum voice --doctor (F2.2)\n");
    let bin = std::env::current_exe().unwrap_or_default();
    let key = hotkey();
    let wayland = std::env::var("WAYLAND_DISPLAY").is_ok();
    let kde = std::env::var("KDE_FULL_SESSION").is_ok()
        || std::env::var("XDG_CURRENT_DESKTOP").map(|d| d.contains("KDE")).unwrap_or(false);
    let toggle_ok = Command::new(&bin)
        .args(["voice", "--status"])
        .env("NEXUM_NO_MARKER", "1")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let rc = shortcut_in_rc(&key);
    let daemon = shortcut_active_in_daemon();
    let conflict = hotkey_conflict(&key);
    let (asr, asr_d) = super::asr_whisper::detect();
    let (tts, tts_d) = super::tts_piper::detect();

    println!("  {} nexum en PATH: {}", ok(in_path("nexum")), if in_path("nexum") { "sí" } else { "no (usá la ruta completa en el atajo — ya lo hace el .desktop)" });
    println!("  ✓ binario real: {}", bin.display());
    println!("  {} `nexum voice --toggle` ejecutable", ok(toggle_ok));
    println!("  {} entorno: {}{}", ok(kde), if kde { "KDE" } else { "no-KDE" }, if wayland { " + Wayland" } else { "" });
    println!("  {} atajo {key} CONFIGURADO en kglobalshortcutsrc", ok(rc));
    println!(
        "  {} atajo ACTIVO en el daemon (kglobalaccel/kwin): {}",
        ok(daemon),
        if daemon { "sí" } else { "no — se activa al reiniciar sesión, o abrí Preferencias→Atajos y tocá Aplicar" }
    );
    match &conflict {
        Some(c) => println!("  ✗ conflicto: {key} también está en {c} — sugeridas: Ctrl+Meta+Z, Meta+Space, F9 (o corré --install-shortcut-kde que lo libera)"),
        None => println!("  ✓ {key} sin conflictos"),
    }
    println!("  {} notificaciones (notify-send)", ok(in_path("notify-send")));
    println!("  {} HUD QML (qml6)", ok(in_path("qml6") || in_path("qml")));
    println!("  {} grabación: {}", ok(super::audio::detect_backend().is_some()), super::audio::detect_backend().unwrap_or("NO"));
    println!("  {} ASR: {asr_d}", ok(asr.is_some()));
    // Verificar que el binario y el modelo EXISTEN no verifica que sinteticen.
    // Con `tts.is_some()` como única prueba, este doctor decía LISTO sin haber
    // emitido un solo byte de audio — y el TTS ni siquiera entraba en el
    // veredicto, así que habría dicho LISTO con Piper ausente.
    let (sintetiza, detalle_synth) = match &tts {
        Some(t) => {
            let out = super::headless::runtime_dir().join(format!(
                "doctor-tts-{}.wav",
                std::process::id()
            ));
            let r = t.synth_probe("Nexum en línea.", &out);
            let _ = std::fs::remove_file(&out);
            match r {
                Ok(n) => (true, format!("sintetiza ({n} bytes de audio)")),
                Err(e) => (false, format!("NO sintetiza — {e}")),
            }
        }
        None => (false, "no se ejecutó (sin TTS)".to_string()),
    };
    // El ✓ tiene que significar "funciona", no "está instalado": con un modelo
    // corrupto esta línea decía `✓ TTS: Piper disponible` igual.
    println!("  {} TTS: {tts_d} — {detalle_synth}", ok(sintetiza));

    let all = listo_para_hablar(
        toggle_ok,
        rc,
        super::audio::detect_backend().is_some(),
        sintetiza,
    );
    println!("\n  Veredicto: {}", if all && daemon { "LISTO — apretá el atajo y hablá" } else if all { "CONFIGURADO — falta activar el atajo (re-login o Preferencias→Atajos→Aplicar)" } else { "INCOMPLETO — revisá los ✗ de arriba" });
    0
}

/// Registro seguro y reversible del atajo en KDE.
pub fn run_install_shortcut() -> i32 {
    let key = hotkey();
    let home = dirs_next::home_dir().unwrap_or_default();
    let rc = shortcuts_rc();
    // 1. Backup.
    if rc.is_file() {
        let bak = home.join(format!(".config/kglobalshortcutsrc.bak-nexum-{}", std::process::id()));
        let _ = std::fs::copy(&rc, &bak);
        println!("backup: {}", bak.display());
    }
    // 2. Liberar el hotkey si otro servicio lo tiene (autorización de Nico
    //    para Super+Z; para otros hotkeys el usuario ya eligió la key).
    if let Some(owner) = hotkey_conflict(&key) {
        if let Some(group) = owner.strip_prefix("[services][").and_then(|s| s.strip_suffix(']')) {
            let _ = Command::new("kwriteconfig6")
                .args(["--file", "kglobalshortcutsrc", "--group", "services", "--group", group, "--key", "_launch", "none"])
                .status();
            println!("liberado {key} de {group}");
        } else {
            println!("⚠ {key} lo usa {owner} (no-service): liberalo desde Preferencias→Atajos");
        }
    }
    // 3. .desktop propio (NoDisplay).
    let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("nexum"));
    let desktop_dir = home.join(".local/share/applications");
    let _ = std::fs::create_dir_all(&desktop_dir);
    let desktop = format!(
        "[Desktop Entry]\nType=Application\nName=Nexum Voz\nComment=Push-to-talk de Nexum (toggle)\nExec={} voice --toggle\nIcon=audio-input-microphone\nNoDisplay=true\nTerminal=false\n",
        bin.display()
    );
    let _ = std::fs::write(desktop_dir.join("nexum-voice.desktop"), desktop);
    // 4. Binding.
    let st = Command::new("kwriteconfig6")
        .args(["--file", "kglobalshortcutsrc", "--group", "services", "--group", "nexum-voice.desktop", "--key", "_launch", &key])
        .status();
    if !st.map(|s| s.success()).unwrap_or(false) {
        eprintln!("kwriteconfig6 falló — configuralo manual (guía docs/user/NEXUM_VOICE_HOTKEY_KDE_SETUP.md)");
        return 1;
    }
    // 5. Intento de recarga suave (si kglobalaccel corre standalone).
    let _ = Command::new("systemctl").args(["--user", "try-restart", "plasma-kglobalaccel.service"]).status();
    println!("atajo {key} → nexum voice --toggle CONFIGURADO.");
    println!("Si el doctor dice 'falta activar': cerrá sesión y volvé a entrar,");
    println!("o abrí Preferencias→Atajos y tocá Aplicar (kwin relee la config).");
    println!("Reversible: restaurá el backup o borrá nexum-voice.desktop.");
    0
}

/// Veredicto del diagnóstico de voz, aparte para poder fijarlo con un test.
///
/// `sintetiza` es el que faltaba: la versión anterior era
/// `toggle_ok && rc && audio`, así que el TTS se IMPRIMÍA pero no entraba en el
/// veredicto. Un Piper ausente o con el modelo corrupto daba LISTO igual.
pub(super) fn listo_para_hablar(
    toggle_ok: bool,
    rc: bool,
    audio: bool,
    sintetiza: bool,
) -> bool {
    toggle_ok && rc && audio && sintetiza
}

#[cfg(test)]
mod tests_veredicto {
    use super::listo_para_hablar;

    #[test]
    fn sin_sintesis_no_hay_veredicto_positivo() {
        // LA propiedad: da igual qué tan bien esté todo lo demás — si el motor
        // no produjo audio, el diagnóstico no puede decir que la voz está lista.
        assert!(!listo_para_hablar(true, true, true, false));
    }

    #[test]
    fn con_todo_en_verde_si() {
        assert!(listo_para_hablar(true, true, true, true));
    }

    #[test]
    fn cada_condicion_es_necesaria() {
        assert!(!listo_para_hablar(false, true, true, true), "sin atajo");
        assert!(!listo_para_hablar(true, false, true, true), "sin rc");
        assert!(!listo_para_hablar(true, true, false, true), "sin grabación");
    }
}
