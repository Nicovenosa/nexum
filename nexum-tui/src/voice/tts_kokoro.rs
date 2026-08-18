//! KokoroAdapter (F2.2.2): TTS natural local vía venv aislado
//! (~/.nexum/engines/kokoro-venv, instalado FUERA del repo). Lazy: el
//! modelo se carga solo al sintetizar (proceso one-shot, reposo 0).
//! CPU-only · sin cloud · timeout duro · temporales limpiados.

use std::path::PathBuf;
use std::process::Command;

pub struct KokoroTts {
    pub venv_python: PathBuf,
    pub model: PathBuf,
    pub voices: PathBuf,
}

pub fn detect() -> (Option<KokoroTts>, String) {
    let base = nexum_agent::config_home::nexum_home().join("engines/kokoro");
    let venv = nexum_agent::config_home::nexum_home().join("engines/kokoro-venv/bin/python");
    let model = base.join("kokoro-v1.0.int8.onnx");
    let voices = base.join("voices-v1.0.bin");
    if venv.is_file() && model.is_file() && voices.is_file() {
        (
            Some(KokoroTts { venv_python: venv, model, voices }),
            "Kokoro disponible (int8, 3 voces es)".into(),
        )
    } else {
        (None, "Kokoro NO instalado (opcional; Piper sigue de fallback)".into())
    }
}

impl KokoroTts {
    /// Sintetiza y reproduce. `speed` del perfil. Timeout 45s (frases
    /// conversacionales; el selector evita Kokoro para textos largos).
    pub fn speak(
        &self,
        text: &str,
        engine_voice_id: &str,
        speed: f32,
        runtime_dir: &std::path::Path,
    ) -> Result<(), String> {
        let wav = runtime_dir.join(format!("tts-k-{}.wav", std::process::id()));
        let script = format!(
            r#"
import sys, soundfile as sf
from kokoro_onnx import Kokoro
k = Kokoro(r"{model}", r"{voices}")
samples, sr = k.create(sys.stdin.read(), voice="{voice}", speed={speed}, lang="es")
sf.write(r"{wav}", samples, sr)
"#,
            model = self.model.display(),
            voices = self.voices.display(),
            voice = engine_voice_id,
            speed = speed.clamp(0.5, 2.0),
            wav = wav.display(),
        );
        let mut child = Command::new("timeout")
            .arg("45")
            .arg(&self.venv_python)
            .args(["-c", &script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("kokoro spawn: {e}"))?;
        {
            use std::io::Write as _;
            let mut si = child.stdin.take().ok_or("kokoro stdin")?;
            si.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
        let st = child.wait().map_err(|e| e.to_string())?;
        if !st.success() {
            let _ = std::fs::remove_file(&wav);
            return Err("kokoro falló (mirá --doctor)".into());
        }
        let player = ["pw-play", "paplay", "aplay"]
            .into_iter()
            .find(|b| {
                std::env::var_os("PATH")
                    .map(|p| std::env::split_paths(&p).any(|d| d.join(b).is_file()))
                    .unwrap_or(false)
            })
            .ok_or("sin reproductor")?;
        let play = Command::new("timeout").arg("30").arg(player).arg(&wav).status();
        let _ = std::fs::remove_file(&wav);
        play.map_err(|e| e.to_string()).and_then(|s| {
            if s.success() { Ok(()) } else { Err("player falló".into()) }
        })
    }
}
