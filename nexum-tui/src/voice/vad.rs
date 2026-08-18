//! VAD simple por RMS (Voice F2.2): lee el WAV que pw-record va
//! escribiendo (s16le mono 16k) y decide cuándo cortar solo.
//!
//! Modelo: Super+Z → escucha → el usuario habla → silencio de N segundos
//! (configurable) → corta y procesa. Sin voz detectada en el arranque →
//! "No escuché nada claro". Interfaz pensada para reemplazar por
//! WebRTC/Silero VAD en F3 sin tocar el caller.

use std::path::Path;

pub struct VadConfig {
    pub silence_secs: f32,     // NEXUM_VOICE_SILENCE_SECONDS (def 2.5)
    pub max_record_secs: u64,  // NEXUM_VOICE_MAX_RECORD_SECONDS (def 60)
    pub threshold: f64,        // NEXUM_VOICE_SILENCE_THRESHOLD RMS (def 700)
    pub min_speech_frames: u32,
}

impl VadConfig {
    pub fn from_env() -> Self {
        let f = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        Self {
            silence_secs: f("NEXUM_VOICE_SILENCE_SECONDS", 2.5).clamp(0.5, 15.0),
            max_record_secs: f("NEXUM_VOICE_MAX_RECORD_SECONDS", super::audio::MAX_RECORD_SECS as f32).clamp(5.0, 300.0) as u64,
            threshold: f("NEXUM_VOICE_SILENCE_THRESHOLD", 700.0).max(50.0) as f64,
            min_speech_frames: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadVerdict {
    WaitingSpeech,     // aún no habló
    Speech,            // hablando
    SilenceCountdown,  // habló y ahora calla (ventana corriendo)
    CutSilence,        // cortar: silencio suficiente tras voz
    CutNoSpeech,       // cortar: nunca habló (timeout de espera)
}

pub struct RmsVad {
    cfg: VadConfig,
    read_pos: u64,
    speech_frames: u32,
    silent_streak: f32, // segundos consecutivos de silencio tras voz
    waited: f32,        // segundos sin voz desde el inicio
}

impl RmsVad {
    pub fn new(cfg: VadConfig) -> Self {
        Self { cfg, read_pos: 44, speech_frames: 0, silent_streak: 0.0, waited: 0.0 }
    }

    /// Poll (~cada `dt` segs): lee bytes nuevos del WAV y actualiza estado.
    pub fn poll(&mut self, wav: &Path, dt: f32) -> VadVerdict {
        let rms = self.read_new_rms(wav).unwrap_or(0.0);
        let speaking = rms >= self.cfg.threshold;
        if speaking {
            self.speech_frames += 1;
            self.silent_streak = 0.0;
        }
        let has_spoken = self.speech_frames >= self.cfg.min_speech_frames;
        if has_spoken {
            if speaking {
                return VadVerdict::Speech;
            }
            self.silent_streak += dt;
            if self.silent_streak >= self.cfg.silence_secs {
                return VadVerdict::CutSilence;
            }
            return VadVerdict::SilenceCountdown;
        }
        self.waited += dt;
        // 12s sin detectar voz → no insistir (privacidad + UX).
        if self.waited >= 12.0 {
            return VadVerdict::CutNoSpeech;
        }
        VadVerdict::WaitingSpeech
    }

    pub fn detected_speech(&self) -> bool {
        self.speech_frames >= self.cfg.min_speech_frames
    }

    fn read_new_rms(&mut self, wav: &Path) -> Option<f64> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(wav).ok()?;
        let len = f.metadata().ok()?.len();
        if len <= self.read_pos {
            return Some(0.0);
        }
        f.seek(SeekFrom::Start(self.read_pos)).ok()?;
        let take = (len - self.read_pos).min(64 * 1024) as usize;
        let mut buf = vec![0u8; take];
        f.read_exact(&mut buf).ok()?;
        self.read_pos += take as u64;
        let mut sum = 0f64;
        let mut n = 0f64;
        for ch in buf.chunks_exact(2) {
            let s = i16::from_le_bytes([ch[0], ch[1]]) as f64;
            sum += s * s;
            n += 1.0;
        }
        (n > 0.0).then(|| (sum / n).sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_with(samples: &[i16], path: &Path) {
        let mut bytes = vec![0u8; 44]; // header dummy (el VAD lo saltea)
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn test_corta_por_silencio_despues_de_voz() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("t.wav");
        let cfg = VadConfig { silence_secs: 1.0, max_record_secs: 60, threshold: 500.0, min_speech_frames: 2 };
        let mut vad = RmsVad::new(cfg);
        wav_with(&[8000; 4000], &wav); // voz fuerte
        assert_eq!(vad.poll(&wav, 0.2), VadVerdict::WaitingSpeech); // frame 1
        wav_with(&[8000; 8000], &wav);
        assert_eq!(vad.poll(&wav, 0.2), VadVerdict::Speech); // frame 2 → habló
        // silencio: no crece el archivo → rms 0
        assert_eq!(vad.poll(&wav, 0.6), VadVerdict::SilenceCountdown);
        assert_eq!(vad.poll(&wav, 0.6), VadVerdict::CutSilence, "1.2s ≥ 1.0s");
        assert!(vad.detected_speech());
    }

    #[test]
    fn test_sin_voz_corta_como_no_speech() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("t.wav");
        wav_with(&[10; 2000], &wav); // casi silencio
        let mut vad = RmsVad::new(VadConfig { silence_secs: 2.0, max_record_secs: 60, threshold: 500.0, min_speech_frames: 2 });
        for _ in 0..19 {
            let v = vad.poll(&wav, 0.65);
            if v == VadVerdict::CutNoSpeech {
                assert!(!vad.detected_speech());
                return;
            }
        }
        panic!("debió cortar por no-speech a los 12s");
    }

    #[test]
    fn test_config_env_con_clamps() {
        let _g = crate::hormiguero::bridge::test_env_lock();
        std::env::set_var("NEXUM_VOICE_SILENCE_SECONDS", "0.1"); // < min
        let c = VadConfig::from_env();
        std::env::remove_var("NEXUM_VOICE_SILENCE_SECONDS");
        assert!(c.silence_secs >= 0.5, "clamp de seguridad");
    }
}
