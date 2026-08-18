//! Exactly-once terminal authority for a single turn.

use std::sync::{Arc, Mutex};

use nexum_agent::agent::AgentCancellationToken;
use serde::{Deserialize, Serialize};

use super::event_sink::EventSink;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TerminalState {
    Completed,
    Failed,
    TimedOut,
    Cancelled,
    RejectedByPolicy,
}

#[derive(Clone)]
pub struct TerminalController {
    state: Arc<Mutex<Option<TerminalState>>>,
    cancel: AgentCancellationToken,
}

impl TerminalController {
    pub fn new(cancel: AgentCancellationToken) -> Self {
        Self {
            state: Arc::new(Mutex::new(None)),
            cancel,
        }
    }

    pub fn state(&self) -> Option<TerminalState> {
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The first caller owns cleanup and notification. Later signals are
    /// ignored, so a provider error followed by EOF cannot emit twice.
    pub async fn finish(
        &self,
        sink: &dyn EventSink,
        session_id: &str,
        state: TerminalState,
        message: Option<&str>,
    ) -> bool {
        {
            let mut current = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if current.is_some() {
                return false;
            }
            *current = Some(state);
        }
        if state != TerminalState::Completed && state != TerminalState::RejectedByPolicy {
            self.cancel.cancel();
        }
        sink.push_terminal(session_id, state, message).await;
        true
    }
}

#[cfg(test)]
#[path = "terminal_test.rs"]
mod tests;
