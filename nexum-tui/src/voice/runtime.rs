//! Entrega local de respuestas de voz sin acoplar el runtime a un motor TTS.

use std::path::Path;

use super::{
    VoiceResponse,
    onboarding::{AcpTextDelivery, VoiceOnboardingAddendum},
    profile::{ProfileLoadError, load_from_path, resolve_local_default_profile, save_to_path},
    tts_backend::TtsBackend,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextDeliveryWarning {
    Disabled,
    Cancelled,
    TtsUnavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDelivery {
    pub text: String,
    pub spoken: bool,
    pub warning: Option<TextDeliveryWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingDeliveryWarning {
    Disabled,
    Cancelled,
    ProfileCorrupt(String),
    ProfileUnreadable(String),
    ProfilePersist(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceRuntimeDelivery {
    pub text: TextDelivery,
    pub addendum: Option<AcpTextDelivery>,
    pub onboarding_warning: Option<OnboardingDeliveryWarning>,
}

pub struct VoiceRuntime<B> {
    enabled: bool,
    cancelled: bool,
    backend: Option<B>,
    onboarding: Option<VoiceOnboardingAddendum>,
}

impl<B: TtsBackend> VoiceRuntime<B> {
    pub fn enabled(backend: Option<B>) -> Self {
        Self {
            enabled: true,
            cancelled: false,
            backend,
            onboarding: None,
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            cancelled: false,
            backend: None,
            onboarding: None,
        }
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    pub fn backend(&self) -> Option<&B> {
        self.backend.as_ref()
    }

    pub fn deliver(&mut self, response: &VoiceResponse) -> TextDelivery {
        // El texto se materializa antes de evaluar TTS: una falla de salida no lo oculta.
        let text = response.text_full.clone();
        if !self.enabled {
            return TextDelivery {
                text,
                spoken: false,
                warning: Some(TextDeliveryWarning::Disabled),
            };
        }
        if self.cancelled {
            return TextDelivery {
                text,
                spoken: false,
                warning: Some(TextDeliveryWarning::Cancelled),
            };
        }
        if !response.should_speak {
            return TextDelivery {
                text,
                spoken: false,
                warning: None,
            };
        }
        match self.backend.as_mut() {
            Some(backend) => match backend.speak(&response.text_speakable) {
                Ok(()) => TextDelivery {
                    text,
                    spoken: true,
                    warning: None,
                },
                Err(error) => TextDelivery {
                    text,
                    spoken: false,
                    warning: Some(TextDeliveryWarning::TtsUnavailable(error)),
                },
            },
            None => TextDelivery {
                text,
                spoken: false,
                warning: None,
            },
        }
    }

    pub fn deliver_with_onboarding(
        &mut self,
        response: &VoiceResponse,
        locale: &str,
        profile_path: &Path,
    ) -> VoiceRuntimeDelivery {
        let text = self.deliver(response);
        let (addendum, onboarding_warning) = if !self.enabled {
            (None, Some(OnboardingDeliveryWarning::Disabled))
        } else if self.cancelled {
            (None, Some(OnboardingDeliveryWarning::Cancelled))
        } else {
            self.deliver_onboarding(locale, profile_path)
        };
        VoiceRuntimeDelivery {
            text,
            addendum,
            onboarding_warning,
        }
    }

    fn deliver_onboarding(
        &mut self,
        locale: &str,
        profile_path: &Path,
    ) -> (Option<AcpTextDelivery>, Option<OnboardingDeliveryWarning>) {
        let profile = match load_from_path(profile_path) {
            Ok(Some(profile)) => profile,
            Ok(None) => {
                let profile = resolve_local_default_profile(locale);
                if let Err(error) = save_to_path(profile_path, &profile) {
                    return (None, Some(OnboardingDeliveryWarning::ProfilePersist(error)));
                }
                profile
            }
            Err(ProfileLoadError::Corrupt(error)) => {
                return (None, Some(OnboardingDeliveryWarning::ProfileCorrupt(error)));
            }
            Err(ProfileLoadError::Read(error)) => {
                return (
                    None,
                    Some(OnboardingDeliveryWarning::ProfileUnreadable(error)),
                );
            }
        };
        let addendum = self
            .onboarding
            .get_or_insert_with(|| VoiceOnboardingAddendum::new(locale, profile.id));
        if addendum.state == super::onboarding::OnboardingAddendumState::Delivered {
            return (None, None);
        }
        (Some(addendum.textual_delivery()), None)
    }
}
