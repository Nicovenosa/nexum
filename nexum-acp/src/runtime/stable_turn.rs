//! Non-relaxable runtime policy for the stable TUI execution profile.

use std::time::Duration;

use thiserror::Error;

use crate::{provider::LlmProvider, task::TaskEnvelopeV1};

pub const ADVANCED_TASK_MESSAGE: &str =
    "Esta tarea requiere el modo avanzado, actualmente deshabilitado en el perfil estable.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StableTurnPolicy {
    max_model_requests: u32,
    max_agent_iterations: u32,
    max_provider_retries: u32,
    max_tool_calls: u32,
    max_mcp_initializations: u32,
    max_nested_agents: u32,
    max_stream_reconnects: u32,
    max_automatic_continuations: u32,
    total_deadline: Duration,
    no_progress_deadline: Duration,
}

impl Default for StableTurnPolicy {
    fn default() -> Self {
        Self {
            max_model_requests: 1,
            max_agent_iterations: 1,
            max_provider_retries: 0,
            max_tool_calls: 0,
            max_mcp_initializations: 0,
            max_nested_agents: 0,
            max_stream_reconnects: 0,
            max_automatic_continuations: 0,
            total_deadline: Duration::from_secs(90),
            no_progress_deadline: Duration::from_secs(30),
        }
    }
}

impl StableTurnPolicy {
    /// Always returns the fixed policy. Environment/config values are
    /// deliberately not consulted and therefore cannot relax the profile.
    pub fn stable() -> Self {
        Self::default()
    }

    pub fn retry_config(&self) -> nexum_agent::llm::RetryConfig {
        debug_assert_eq!(self.max_provider_retries, 0);
        nexum_agent::llm::RetryConfig::one_shot()
    }

    pub fn max_model_requests(&self) -> u32 {
        self.max_model_requests
    }
    pub fn max_agent_iterations(&self) -> u32 {
        self.max_agent_iterations
    }
    pub fn max_provider_retries(&self) -> u32 {
        self.max_provider_retries
    }
    pub fn max_tool_calls(&self) -> u32 {
        self.max_tool_calls
    }
    pub fn max_mcp_initializations(&self) -> u32 {
        self.max_mcp_initializations
    }
    pub fn max_nested_agents(&self) -> u32 {
        self.max_nested_agents
    }
    pub fn max_stream_reconnects(&self) -> u32 {
        self.max_stream_reconnects
    }
    pub fn max_automatic_continuations(&self) -> u32 {
        self.max_automatic_continuations
    }
    pub fn total_deadline(&self) -> Duration {
        self.total_deadline
    }
    pub fn no_progress_deadline(&self) -> Duration {
        self.no_progress_deadline
    }

    #[cfg(test)]
    pub(crate) fn with_test_deadlines(
        no_progress_deadline: Duration,
        total_deadline: Duration,
    ) -> Self {
        Self {
            no_progress_deadline,
            total_deadline,
            ..Self::stable()
        }
    }

    pub fn validate_envelope(
        &self,
        envelope: &TaskEnvelopeV1,
        provider: &LlmProvider,
    ) -> Result<StableFlow, StablePolicyError> {
        if envelope.envelope_id.trim().is_empty()
            || envelope.objective.trim().is_empty()
            || envelope.user_input.trim().is_empty()
            || envelope.session_id.trim().is_empty()
            || envelope.thread_id.trim().is_empty()
        {
            return Err(StablePolicyError::MissingRequiredEnvelopeField);
        }
        if !envelope.allowed_tools.is_empty() {
            return Err(StablePolicyError::ToolsNotAllowed);
        }
        let meta = &envelope.sanitized_metadata;
        for key in [
            "trace_id",
            "turn_id",
            "request_id",
            "route_decision",
            "task_classification",
            "selected_provider",
            "selected_model",
        ] {
            if meta.get(key).is_none_or(|value| value.trim().is_empty()) {
                return Err(StablePolicyError::MissingMetadata(key));
            }
        }

        let selected_provider = &meta["selected_provider"];
        let selected_model = &meta["selected_model"];
        if selected_provider != provider.display_name() || selected_model != provider.model_name() {
            return Err(StablePolicyError::ProviderModelMismatch {
                selected_provider: selected_provider.clone(),
                selected_model: selected_model.clone(),
                actual_provider: provider.display_name().to_string(),
                actual_model: provider.model_name().to_string(),
            });
        }

        match (
            meta["route_decision"].as_str(),
            meta["task_classification"].as_str(),
        ) {
            ("one_shot", "simple") => Ok(StableFlow::OneShot),
            ("rejected_by_policy", "advanced") => Ok(StableFlow::RejectedByPolicy),
            (decision, classification) => Err(StablePolicyError::InvalidDecision {
                decision: decision.to_string(),
                classification: classification.to_string(),
            }),
        }
    }

    pub fn require_envelope<'a>(
        &self,
        envelope: Option<&'a TaskEnvelopeV1>,
    ) -> Result<&'a TaskEnvelopeV1, StablePolicyError> {
        envelope.ok_or(StablePolicyError::MissingEnvelope)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StableFlow {
    OneShot,
    RejectedByPolicy,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StablePolicyError {
    #[error("stable profile requires taskEnvelope")]
    MissingEnvelope,
    #[error("stable task envelope is missing a required field")]
    MissingRequiredEnvelopeField,
    #[error("stable task envelope is missing required metadata: {0}")]
    MissingMetadata(&'static str),
    #[error("tools are forbidden in the stable profile")]
    ToolsNotAllowed,
    #[error(
        "selected provider/model changed: selected={selected_provider}/{selected_model}, actual={actual_provider}/{actual_model}"
    )]
    ProviderModelMismatch {
        selected_provider: String,
        selected_model: String,
        actual_provider: String,
        actual_model: String,
    },
    #[error("invalid stable decision/classification: {decision}/{classification}")]
    InvalidDecision {
        decision: String,
        classification: String,
    },
}

#[cfg(test)]
#[path = "stable_turn_test.rs"]
mod tests;
