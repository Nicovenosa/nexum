//! Contrato de salida TTS independiente del motor.
//! Los adaptadores reales quedan fuera de este checkpoint.

pub trait TtsBackend: Send {
    fn backend_name(&self) -> &'static str;
    fn speak(&mut self, text: &str) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct FakeTtsBackend {
    spoken: Vec<String>,
    failure: Option<String>,
}

impl FakeTtsBackend {
    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            spoken: Vec::new(),
            failure: Some(message.into()),
        }
    }

    pub fn spoken(&self) -> &[String] {
        &self.spoken
    }
}

impl TtsBackend for FakeTtsBackend {
    fn backend_name(&self) -> &'static str {
        "fake-tts"
    }

    fn speak(&mut self, text: &str) -> Result<(), String> {
        if let Some(message) = &self.failure {
            return Err(message.clone());
        }
        self.spoken.push(text.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deliver(backend: &mut dyn TtsBackend, text: &str) -> Result<(), String> {
        backend.speak(text)
    }

    #[test]
    fn test_fake_backend_entrega_texto_sin_motor_real() {
        let mut backend = FakeTtsBackend::default();
        assert_eq!(backend.backend_name(), "fake-tts");
        deliver(&mut backend, "respuesta corta").unwrap();
        assert_eq!(backend.spoken(), &["respuesta corta"]);
    }

    #[test]
    fn test_fake_backend_propaga_falla_configurada() {
        let mut backend = FakeTtsBackend::failing("salida no disponible");
        assert_eq!(
            deliver(&mut backend, "no se emite"),
            Err("salida no disponible".into())
        );
        assert!(backend.spoken().is_empty());
    }
}
