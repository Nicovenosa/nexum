use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nexum_agent::agent::{events::AgentEvent, AgentCancellationToken};

use super::*;

#[derive(Default)]
struct MockSink {
    terminal_count: Mutex<usize>,
}

#[async_trait]
impl EventSink for MockSink {
    async fn push_event(&self, _session_id: &str, _event: &AgentEvent, _context_window: u32) {}

    async fn push_done(&self, _session_id: &str) {}

    async fn push_terminal(
        &self,
        _session_id: &str,
        _state: TerminalState,
        _message: Option<&str>,
    ) {
        *self.terminal_count.lock().unwrap() += 1;
    }
}

#[tokio::test]
async fn duplicate_terminal_is_ignored() {
    let sink = Arc::new(MockSink::default());
    let controller = TerminalController::new(AgentCancellationToken::new());
    assert!(
        controller
            .finish(sink.as_ref(), "s", TerminalState::Failed, Some("first"))
            .await
    );
    assert!(
        !controller
            .finish(sink.as_ref(), "s", TerminalState::Completed, None)
            .await
    );
    assert_eq!(controller.state(), Some(TerminalState::Failed));
    assert_eq!(*sink.terminal_count.lock().unwrap(), 1);
}
