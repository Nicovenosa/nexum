//! Grabación de audio local (Voice F2) — spawn de grabadores del sistema
//! (pw-record > parecord > arecord), WAV 16k mono a archivo temporal en el
//! runtime dir (tmpfs, 0700). El audio JAMÁS se persiste por defecto: el
//! WAV se borra apenas se transcribe (o al fallar). Nunca cloud.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Timeout duro de seguridad: ninguna escucha dura más que esto.
pub const MAX_RECORD_SECS: u64 = 60;

pub struct Recorder {
    child: Child,
    pub wav_path: PathBuf,
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

/// Backend disponible (para /voz engines y status).
pub fn detect_backend() -> Option<&'static str> {
    ["pw-record", "parecord", "arecord"]
        .into_iter()
        .find(|b| which(b))
}

impl Recorder {
    /// Arranca la grabación. El indicador visible DEBE estar activo antes
    /// de llamar acá (invariante: nunca grabar sin overlay).
    pub fn start(runtime_dir: &Path) -> Result<Self, String> {
        let backend = detect_backend().ok_or_else(|| {
            "sin grabador de audio (pw-record/parecord/arecord)".to_string()
        })?;
        let wav_path = runtime_dir.join(format!("voice-{}.wav", std::process::id()));
        let args: Vec<String> = match backend {
            "pw-record" => vec![
                "--rate".into(), "16000".into(),
                "--channels".into(), "1".into(),
                "--format".into(), "s16".into(),
                wav_path.display().to_string(),
            ],
            "parecord" => vec![
                "--rate=16000".into(), "--channels=1".into(),
                "--format=s16le".into(), "--file-format=wav".into(),
                wav_path.display().to_string(),
            ],
            _ => vec![
                "-q".into(), "-f".into(), "S16_LE".into(),
                "-r".into(), "16000".into(), "-c".into(), "1".into(),
                wav_path.display().to_string(),
            ],
        };
        let child = Command::new(backend)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("no pude arrancar {backend}: {e}"))?;
        Ok(Self { child, wav_path })
    }

    /// Corta la grabación (SIGTERM al grabador → WAV finalizado) y devuelve
    /// el path del WAV si hay contenido.
    pub fn stop(mut self) -> Result<PathBuf, String> {
        // SIGTERM: pw-record/arecord cierran el WAV correctamente.
        unsafe { libc_kill(self.child.id() as i32) };
        let _ = self.child.wait();
        let size = std::fs::metadata(&self.wav_path).map(|m| m.len()).unwrap_or(0);
        if size < 128 {
            let _ = std::fs::remove_file(&self.wav_path);
            return Err("no se capturó audio (grabación vacía)".to_string());
        }
        Ok(self.wav_path.clone())
    }

    /// Cancela y descarta el audio (privacidad: borrar siempre).
    pub fn cancel(mut self) {
        unsafe { libc_kill(self.child.id() as i32) };
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.wav_path);
    }
}

/// SIGTERM sin dependencias nuevas (libc ya está en el árbol vía deps).
#[cfg(unix)]
unsafe fn libc_kill(pid: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid, 15) };
}

#[cfg(not(unix))]
unsafe fn libc_kill(_pid: i32) {}

/// Borra el WAV — se llama SIEMPRE tras transcribir (ok o error).
pub fn discard_wav(path: &Path) {
    let _ = std::fs::remove_file(path);
}
