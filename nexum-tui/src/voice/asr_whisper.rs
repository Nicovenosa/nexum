//! ASR local real (Voice F2): whisper.cpp vía binario del sistema.
//! Sin cloud. Si no está instalado/configurado, degrada limpio con
//! instrucciones — jamás rompe el flujo.

use std::path::PathBuf;
use std::process::Command;

pub struct WhisperCpp {
    pub bin: PathBuf,
    pub model: PathBuf,
}

fn find_bin() -> Option<PathBuf> {
    if let Ok(b) = std::env::var("NEXUM_WHISPER_BIN") {
        let p = PathBuf::from(b);
        if p.is_file() {
            return Some(p);
        }
    }
    for name in ["whisper-cli", "whisper-cpp", "whisper.cpp", "whisper"] {
        if let Some(path) = std::env::var_os("PATH").and_then(|p| {
            std::env::split_paths(&p).map(|d| d.join(name)).find(|c| c.is_file())
        }) {
            return Some(path);
        }
    }
    None
}

fn find_model() -> Option<PathBuf> {
    if let Ok(m) = std::env::var("NEXUM_WHISPER_MODEL") {
        let p = PathBuf::from(m);
        if p.is_file() {
            return Some(p);
        }
    }
    let dir = nexum_agent::config_home::nexum_home().join("models/whisper");
    std::fs::read_dir(&dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        (p.extension().is_some_and(|x| x == "bin")).then_some(p)
    })
}

/// Estado de detección (para status/engines): (disponible, detalle público).
pub fn detect() -> (Option<WhisperCpp>, String) {
    match (find_bin(), find_model()) {
        (Some(bin), Some(model)) => {
            let d = format!(
                "whisper.cpp disponible ({})",
                model.file_name().unwrap_or_default().to_string_lossy()
            );
            (Some(WhisperCpp { bin, model }), d)
        }
        (None, _) => (None, "whisper.cpp NO instalado — instalalo y/o seteá NEXUM_WHISPER_BIN".into()),
        (Some(_), None) => (None,
            "whisper.cpp instalado pero SIN modelo — descargá un ggml-*.bin a ~/.nexum/models/whisper/ o seteá NEXUM_WHISPER_MODEL".into()),
    }
}

impl WhisperCpp {
    /// Transcribe un WAV 16k mono. Timeout duro vía `timeout(1)` del sistema.
    /// PRIVACIDAD: no loggea el contenido; el caller borra el WAV siempre.
    pub fn transcribe(&self, wav: &std::path::Path, lang: &str) -> Result<String, String> {
        let out = Command::new("timeout")
            .arg("90")
            .arg(&self.bin)
            .args(["-m"])
            .arg(&self.model)
            .args(["-l", lang, "-nt", "--no-prints"])
            // Prompt de contexto: sesga el vocabulario hacia los comandos
            // de Nexum (mejora es-AR: "hablá", "Nexum", estilos). Técnica
            // estándar de whisper, no altera frases normales.
            .args(["--prompt", "Nexum. Hola Nexum, ¿estás? Hablá más serio. Hablá más suave. Hablá más grave. Hablá más rápido. Hablá más lento. Volvé a tu voz normal. Quiero una voz más grave. Quiero una voz más cálida. No me gusta tu voz. Probame otra voz. Volvé a la voz anterior."])
            .args(["-f"])
            .arg(wav)
            .output()
            .map_err(|e| format!("whisper spawn: {e}"))?;
        if !out.status.success() {
            return Err(format!("whisper exit {:?}", out.status.code()));
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if text.is_empty() {
            return Err("transcripción vacía (¿hablaste?)".to_string());
        }
        Ok(text)
    }
}
