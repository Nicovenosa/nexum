use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use nexum_agent::{
    agent::{events::AgentEvent, AgentCancellationToken},
    error::{AgentError, AgentResult},
    llm::{BaseModel, LlmRequest, LlmResponse, StopReason},
    messages::{BaseMessage, MessageContent},
};

use super::*;
use crate::session::terminal::TerminalState;

enum MockOutcome {
    Success,
    RetryableError,
    Silent,
}

struct MockModel {
    calls: Arc<AtomicUsize>,
    outcome: MockOutcome,
}

#[async_trait]
impl BaseModel for MockModel {
    async fn invoke(&self, _request: LlmRequest) -> AgentResult<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.outcome {
            MockOutcome::Success => Ok(LlmResponse {
                message: BaseMessage::ai("respuesta"),
                stop_reason: StopReason::EndTurn,
                usage: None,
                request_id: None,
            }),
            MockOutcome::RetryableError => Err(AgentError::LlmHttpError {
                status: 429,
                message: "rate limited".into(),
            }),
            MockOutcome::Silent => std::future::pending().await,
        }
    }

    fn provider_name(&self) -> &str {
        "MockProvider"
    }

    fn model_id(&self) -> &str {
        "mock-model"
    }
}

#[derive(Default)]
struct MockSink {
    terminal: Mutex<Vec<TerminalState>>,
    events: Mutex<Vec<String>>,
}

#[async_trait]
impl EventSink for MockSink {
    async fn push_event(&self, _session_id: &str, event: &AgentEvent, _context_window: u32) {
        self.events
            .lock()
            .unwrap()
            .push(serde_json::to_string(event).unwrap());
    }

    async fn push_done(&self, _session_id: &str) {}

    async fn push_terminal(&self, _session_id: &str, state: TerminalState, _message: Option<&str>) {
        self.terminal.lock().unwrap().push(state);
    }
}

fn request(model: MockModel, sink: Arc<MockSink>) -> OneShotRequest {
    OneShotRequest {
        model: Box::new(model),
        provider_name: "MockProvider".into(),
        model_name: "mock-model".into(),
        content: MessageContent::text("Hola"),
        history: vec![],
        session_id: "session-1".into(),
        system_prompt: "directo".into(),
        context_window: 4096,
        cancel: AgentCancellationToken::new(),
        event_sink: sink,
    }
}

#[tokio::test]
async fn stable_simple_turn_makes_exactly_one_request() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(MockSink::default());
    let result = OneShotExecutor::new(StableTurnPolicy::stable())
        .execute(
            StableFlow::OneShot,
            request(
                MockModel {
                    calls: calls.clone(),
                    outcome: MockOutcome::Success,
                },
                sink.clone(),
            ),
        )
        .await;

    assert!(result.ok);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*sink.terminal.lock().unwrap(), [TerminalState::Completed]);
    assert!(
        sink.events
            .lock()
            .unwrap()
            .iter()
            .all(|event| !event.contains("tool_start")),
        "one-shot must expose no tools"
    );
}

#[tokio::test]
async fn advanced_turn_makes_zero_provider_requests_and_no_agentic_initialization() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(MockSink::default());
    let result = OneShotExecutor::new(StableTurnPolicy::stable())
        .execute(
            StableFlow::RejectedByPolicy,
            request(
                MockModel {
                    calls: calls.clone(),
                    outcome: MockOutcome::Success,
                },
                sink.clone(),
            ),
        )
        .await;

    assert!(result.ok);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        result.messages.last().unwrap().content(),
        ADVANCED_TASK_MESSAGE
    );
    assert_eq!(
        *sink.terminal.lock().unwrap(),
        [TerminalState::RejectedByPolicy]
    );
}

#[tokio::test]
async fn provider_fallback_does_not_occur_after_retryable_failure() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(MockSink::default());
    let result = OneShotExecutor::new(StableTurnPolicy::stable())
        .execute(
            StableFlow::OneShot,
            request(
                MockModel {
                    calls: calls.clone(),
                    outcome: MockOutcome::RetryableError,
                },
                sink.clone(),
            ),
        )
        .await;

    assert!(!result.ok);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*sink.terminal.lock().unwrap(), [TerminalState::Failed]);
}

#[tokio::test]
async fn provider_silence_reaches_one_timed_out_terminal() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(MockSink::default());
    let policy = StableTurnPolicy::with_test_deadlines(
        std::time::Duration::from_millis(10),
        std::time::Duration::from_millis(50),
    );
    let result = OneShotExecutor::new(policy)
        .execute(
            StableFlow::OneShot,
            request(
                MockModel {
                    calls: calls.clone(),
                    outcome: MockOutcome::Silent,
                },
                sink.clone(),
            ),
        )
        .await;

    assert!(!result.ok);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*sink.terminal.lock().unwrap(), [TerminalState::TimedOut]);
}

#[tokio::test]
async fn provider_total_deadline_reaches_one_timed_out_terminal() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(MockSink::default());
    let policy = StableTurnPolicy::with_test_deadlines(
        std::time::Duration::from_millis(50),
        std::time::Duration::from_millis(10),
    );
    let result = OneShotExecutor::new(policy)
        .execute(
            StableFlow::OneShot,
            request(
                MockModel {
                    calls: calls.clone(),
                    outcome: MockOutcome::Silent,
                },
                sink.clone(),
            ),
        )
        .await;

    assert!(!result.ok);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*sink.terminal.lock().unwrap(), [TerminalState::TimedOut]);
}

#[tokio::test]
async fn cancellation_before_provider_makes_zero_requests() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(MockSink::default());
    let req = request(
        MockModel {
            calls: calls.clone(),
            outcome: MockOutcome::Success,
        },
        sink.clone(),
    );
    req.cancel.cancel();
    let result = OneShotExecutor::new(StableTurnPolicy::stable())
        .execute(StableFlow::OneShot, req)
        .await;

    assert!(!result.ok);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(*sink.terminal.lock().unwrap(), [TerminalState::Cancelled]);
}

#[tokio::test]
async fn controlled_demo_canary_five_active_turn_cancellations_are_terminal() {
    let mut total_requests = 0_usize;

    for turn in 0..5 {
        let calls = Arc::new(AtomicUsize::new(0));
        let sink = Arc::new(MockSink::default());
        let req = request(
            MockModel {
                calls: calls.clone(),
                outcome: MockOutcome::Silent,
            },
            sink.clone(),
        );
        let cancel = req.cancel.clone();
        let task = tokio::spawn(async move {
            OneShotExecutor::new(StableTurnPolicy::stable())
                .execute(StableFlow::OneShot, req)
                .await
        });

        for _ in 0..100 {
            if calls.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "turn {turn} did not start exactly one provider request"
        );
        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("cancelled turn must not hang")
            .unwrap();

        assert!(!result.ok);
        assert_eq!(*sink.terminal.lock().unwrap(), [TerminalState::Cancelled]);
        total_requests += calls.load(Ordering::SeqCst);
    }

    assert_eq!(total_requests, 5);
}
