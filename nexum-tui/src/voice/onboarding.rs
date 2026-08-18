//! Estado local para agregar el bootstrap de voz al texto de onboarding.
//! No registra metodos ACP: reutiliza los paths de sesion ya soportados.

use serde::{Deserialize, Serialize};

pub const ACP_SESSION_NEW_PATH: &str = "session/new";
pub const ACP_SESSION_PROMPT_PATH: &str = "session/prompt";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnboardingAddendumState {
    Pending,
    Delivered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceOnboardingAddendum {
    pub locale: String,
    pub profile_id: String,
    pub state: OnboardingAddendumState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpTextDelivery {
    pub path: &'static str,
    pub text: String,
}

impl VoiceOnboardingAddendum {
    pub fn new(locale: impl Into<String>, profile_id: impl Into<String>) -> Self {
        Self {
            locale: locale.into(),
            profile_id: profile_id.into(),
            state: OnboardingAddendumState::Pending,
        }
    }

    /// El estado se adjunta a una sesion nueva, sin agregar un RPC propio.
    pub fn state_path(&self) -> &'static str {
        ACP_SESSION_NEW_PATH
    }

    /// Devuelve el texto que un caller existente puede enviar por session/prompt.
    pub fn textual_delivery(&mut self) -> AcpTextDelivery {
        self.state = OnboardingAddendumState::Delivered;
        AcpTextDelivery {
            path: ACP_SESSION_PROMPT_PATH,
            text: format!(
                "Voz local configurada para {} con el perfil {}.",
                self.locale, self.profile_id
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addendum_entrega_texto_por_paths_acp_existentes() {
        let mut addendum = VoiceOnboardingAddendum::new("es-AR", "nexum_default");
        assert_eq!(addendum.state, OnboardingAddendumState::Pending);
        assert_eq!(addendum.state_path(), ACP_SESSION_NEW_PATH);
        let delivery = addendum.textual_delivery();
        assert_eq!(delivery.path, ACP_SESSION_PROMPT_PATH);
        assert!(delivery.text.contains("es-AR"));
        assert!(delivery.text.contains("nexum_default"));
        assert_eq!(addendum.state, OnboardingAddendumState::Delivered);
    }

    #[test]
    fn test_addendum_es_serializable_sin_ruta_onboarding_propia() {
        let addendum = VoiceOnboardingAddendum::new("es-AR", "nexum_default");
        let json = serde_json::to_string(&addendum).unwrap();
        assert!(json.contains("nexum_default"));
        assert!(!json.contains("onboarding/"));
    }
}
