//! TTS local opcional (Voice F2): Piper. Voz preferida es_AR-daniela-high
//! (la que Nico ya usa en PiperRem). Si no está: degradar sin romper.
//! Nunca lee secretos: el caller pasa text_speakable (ya redactado).

use std::path::PathBuf;
use std::process::{Command, Stdio};

pub struct PiperTts {
    pub bin: PathBuf,
    pub voice: PathBuf,
}

fn find_bin() -> Option<PathBuf> {
    if let Ok(b) = std::env::var("NEXUM_PIPER_BIN") {
        let p = PathBuf::from(b);
        if p.is_file() {
            return Some(p);
        }
    }
    for cand in ["/usr/bin/piper-tts", "/opt/piper-tts/piper"] {
        let p = PathBuf::from(cand);
        if p.is_file() {
            return Some(p);
        }
    }
    for name in ["piper-tts", "piper"] {
        if let Some(path) = std::env::var_os("PATH").and_then(|p| {
            std::env::split_paths(&p).map(|d| d.join(name)).find(|c| c.is_file())
        }) {
            return Some(path);
        }
    }
    None
}

fn find_voice() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("NEXUM_PIPER_VOICE") {
        let p = PathBuf::from(v);
        if p.is_file() {
            return Some(p);
        }
    }
    // VoiceProfile (~/.nexum/voice.json): la voz es configurable, Daniela
    // es solo el default (baseline low-cost: cualquier .onnx liviano).
    if let Some(p) = super::profile::resolve_voice_path(&super::profile::load()) {
        return Some(p);
    }
    let home = dirs_next::home_dir()?;
    let preferred = home.join(".local/share/piper/voices/es_AR-daniela-high.onnx");
    if preferred.is_file() {
        return Some(preferred);
    }
    let dir = home.join(".local/share/piper/voices");
    std::fs::read_dir(&dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        (p.extension().is_some_and(|x| x == "onnx")).then_some(p)
    })
}

fn find_player() -> Option<&'static str> {
    ["pw-play", "paplay", "aplay"].into_iter().find(|b| {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join(b).is_file()))
            .unwrap_or(false)
    })
}

pub fn detect() -> (Option<PiperTts>, String) {
    match (find_bin(), find_voice()) {
        (Some(bin), Some(voice)) => {
            let d = format!(
                "Piper disponible ({})",
                voice.file_stem().unwrap_or_default().to_string_lossy()
            );
            (Some(PiperTts { bin, voice }), d)
        }
        (None, _) => (None, "Piper NO instalado (TTS deshabilitado — respuesta solo texto)".into()),
        (Some(_), None) => (None, "Piper instalado pero sin voz .onnx (seteá NEXUM_PIPER_VOICE)".into()),
    }
}

impl PiperTts {
    /// Habla texto corto (bloqueante, con timeout duro). El texto llega ya
    /// redactado (make_speakable). WAV temporal en runtime dir, borrado al
    /// terminar.
    pub fn speak(&self, text: &str, runtime_dir: &std::path::Path) -> Result<(), String> {
        let player = find_player().ok_or("sin reproductor de audio")?;
        let wav = runtime_dir.join(format!("tts-{}.wav", std::process::id()));
        let speed = super::profile::load().speed.clamp(0.5, 2.0);
        let length_scale = format!("{:.2}", 1.0 / speed);
        let mut piper = Command::new("timeout")
            .arg("30")
            .arg(&self.bin)
            .args(["--model"])
            .arg(&self.voice)
            .args(["--length_scale", &length_scale])
            .args(["--output_file"])
            .arg(&wav)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("piper spawn: {e}"))?;
        {
            use std::io::Write as _;
            let mut stdin = piper.stdin.take().ok_or("piper stdin")?;
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("piper write: {e}"))?;
        }
        let st = piper.wait().map_err(|e| format!("piper wait: {e}"))?;
        if !st.success() {
            let _ = std::fs::remove_file(&wav);
            return Err("piper falló".to_string());
        }
        let play = Command::new("timeout")
            .arg("30")
            .arg(player)
            .arg(&wav)
            .status();
        let _ = std::fs::remove_file(&wav);
        play.map_err(|e| format!("player: {e}")).and_then(|s| {
            if s.success() { Ok(()) } else { Err("player falló".into()) }
        })
    }

    /// Sintetiza a un archivo y devuelve su tamaño, SIN reproducir.
    ///
    /// Existe para que el diagnóstico pueda comprobar que el motor produce
    /// audio de verdad. Verificar que el binario y el modelo EXISTEN no
    /// verifica que sinteticen: es el mismo patrón que Doctor aprobando
    /// instalaciones que los gates rechazaban, y el que hizo que
    /// `voice --doctor` dijera LISTO sin haber emitido un solo byte.
    pub fn synth_probe(&self, text: &str, out: &std::path::Path) -> Result<u64, String> {
        self.synth_probe_con_voz(text, &self.voice, out)
    }

    /// Igual, con la voz EXPLÍCITA.
    ///
    /// `speak` usa la voz que eligió `detect()`, no la de la entrada de
    /// catálogo que se está reproduciendo. Con una sola entrada Piper las dos
    /// coinciden y el defecto no se ve; con la segunda, `--preview` anuncia un
    /// perfil y suena el otro. Es la misma clase de siempre: dos lugares para
    /// la misma verdad, esperando a que uno cambie.
    pub fn synth_probe_con_voz(
        &self,
        text: &str,
        voz: &std::path::Path,
        out: &std::path::Path,
    ) -> Result<u64, String> {
        let speed = super::profile::load().speed.clamp(0.5, 2.0);
        let length_scale = format!("{:.2}", 1.0 / speed);
        let mut piper = Command::new("timeout")
            .arg("30")
            .arg(&self.bin)
            .args(["--model"])
            .arg(voz)
            .args(["--length_scale", &length_scale])
            .args(["--output_file"])
            .arg(out)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("piper spawn: {e}"))?;
        {
            use std::io::Write as _;
            let mut stdin = piper.stdin.take().ok_or("piper stdin")?;
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("piper write: {e}"))?;
        }
        let st = piper.wait().map_err(|e| format!("piper wait: {e}"))?;
        if !st.success() {
            return Err("piper terminó con error".to_string());
        }
        let n = std::fs::metadata(out)
            .map_err(|e| format!("el wav no existe: {e}"))?
            .len();
        if n == 0 {
            return Err("piper creó el archivo pero no escribió audio".to_string());
        }
        Ok(n)
    }

}
