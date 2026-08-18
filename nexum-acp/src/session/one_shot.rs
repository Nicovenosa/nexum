//! Stable executor that performs one model request without constructing ReAct.

use std::sync::Arc;

use nexum_agent::{
    agent::{events::AgentEvent as ExecutorEvent, AgentCancellationToken, ReactLLM},
    llm::{BaseModelReactLLM, RetryableLLM},
    messages::{BaseMessage, MessageContent},
};

use super::{
    event_sink::EventSink,
    executor::{PromptResult, PromptStopReason},
    terminal::{TerminalController, TerminalState},
};
use crate::runtime::stable_turn::{StableFlow, StableTurnPolicy, ADVANCED_TASK_MESSAGE};

pub struct OneShotRequest {
    pub model: Box<dyn nexum_agent::llm::BaseModel>,
    pub provider_name: String,
    pub model_name: String,
    pub content: MessageContent,
    pub history: Vec<BaseMessage>,
    pub session_id: String,
    pub system_prompt: String,
    pub context_window: u32,
    pub cancel: AgentCancellationToken,
    pub event_sink: Arc<dyn EventSink>,
}

pub struct OneShotExecutor {
    policy: StableTurnPolicy,
}

impl OneShotExecutor {
    pub fn new(policy: StableTurnPolicy) -> Self {
        Self { policy }
    }

    pub async fn execute(self, flow: StableFlow, req: OneShotRequest) -> PromptResult {
        tracing::debug!(
            provider = %req.provider_name,
            model = %req.model_name,
            ?flow,
            "stable one-shot executor selected"
        );
        let terminal = TerminalController::new(req.cancel.clone());
        let mut messages = req.history;
        messages.push(BaseMessage::human(req.content));

        if flow == StableFlow::RejectedByPolicy {
            let response = BaseMessage::ai(ADVANCED_TASK_MESSAGE);
            let message_id = response.id();
            messages.push(response.clone());
            req.event_sink
                .push_event(
                    &req.session_id,
                    &ExecutorEvent::TextChunk {
                        message_id,
                        chunk: ADVANCED_TASK_MESSAGE.to_string(),
                        source_agent_id: None,
                    },
                    req.context_window,
                )
                .await;
            req.event_sink
                .push_event(
                    &req.session_id,
                    &ExecutorEvent::StateSnapshot(messages.clone()),
                    req.context_window,
                )
                .await;
            terminal
                .finish(
                    req.event_sink.as_ref(),
                    &req.session_id,
                    TerminalState::RejectedByPolicy,
                    Some(ADVANCED_TASK_MESSAGE),
                )
                .await;
            return PromptResult {
                messages,
                ok: true,
                stop_reason: PromptStopReason::EndTurn,
                recall_items: Vec::new(),
            };
        }

        if req.cancel.is_cancelled() {
            return fail(
                messages,
                terminal,
                req.event_sink,
                req.session_id,
                "Turno cancelado.",
                TerminalState::Cancelled,
            )
            .await;
        }

        debug_assert_eq!(self.policy.max_model_requests(), 1);
        let request_snapshot = Arc::new(messages.clone());
        req.event_sink
            .push_event(
                &req.session_id,
                &ExecutorEvent::LlmCallStart {
                    step: 1,
                    messages: request_snapshot,
                    tools: Vec::new(),
                },
                req.context_window,
            )
            .await;

        let llm = BaseModelReactLLM::new(req.model)
            .with_system(req.system_prompt)
            .with_session_id(req.session_id.clone());
        let llm = RetryableLLM::new(llm, self.policy.retry_config());
        let call = llm.generate_reasoning(&messages, &[], None);
        let outcome = tokio::select! {
            _ = req.cancel.cancelled() => OneShotOutcome::Cancelled,
            result = tokio::time::timeout(self.policy.no_progress_deadline(), call) => {
                match result {
                    Ok(Ok(reasoning)) => OneShotOutcome::Response(reasoning),
                    Ok(Err(error)) => OneShotOutcome::Failed(error.to_string()),
                    Err(_) => OneShotOutcome::TimedOut,
                }
            }
            _ = tokio::time::sleep(self.policy.total_deadline()) => OneShotOutcome::TimedOut,
        };

        match outcome {
            OneShotOutcome::Response(reasoning) if reasoning.tool_calls.is_empty() => {
                let answer = reasoning.final_answer.unwrap_or_default();
                let response = reasoning
                    .source_message
                    .unwrap_or_else(|| BaseMessage::ai(answer.clone()));
                let message_id = response.id();
                messages.push(response);
                req.event_sink
                    .push_event(
                        &req.session_id,
                        &ExecutorEvent::LlmCallEnd {
                            step: 1,
                            model: req.model_name,
                            output: answer.clone(),
                            usage: reasoning.usage,
                            stop_reason: Some(reasoning.stop_reason),
                        },
                        req.context_window,
                    )
                    .await;
                req.event_sink
                    .push_event(
                        &req.session_id,
                        &ExecutorEvent::TextChunk {
                            message_id,
                            chunk: answer,
                            source_agent_id: None,
                        },
                        req.context_window,
                    )
                    .await;
                req.event_sink
                    .push_event(
                        &req.session_id,
                        &ExecutorEvent::StateSnapshot(messages.clone()),
                        req.context_window,
                    )
                    .await;
                terminal
                    .finish(
                        req.event_sink.as_ref(),
                        &req.session_id,
                        TerminalState::Completed,
                        None,
                    )
                    .await;
                PromptResult {
                    messages,
                    ok: true,
                    stop_reason: PromptStopReason::EndTurn,
                    recall_items: Vec::new(),
                }
            }
            OneShotOutcome::Response(_) => {
                fail(
                    messages,
                    terminal,
                    req.event_sink,
                    req.session_id,
                    "El provider solicitó herramientas, prohibidas en el perfil estable.",
                    TerminalState::Failed,
                )
                .await
            }
            OneShotOutcome::Failed(error) => {
                fail(
                    messages,
                    terminal,
                    req.event_sink,
                    req.session_id,
                    &error,
                    TerminalState::Failed,
                )
                .await
            }
            OneShotOutcome::TimedOut => {
                fail(
                    messages,
                    terminal,
                    req.event_sink,
                    req.session_id,
                    "El provider no produjo una respuesta dentro del plazo estable.",
                    TerminalState::TimedOut,
                )
                .await
            }
            OneShotOutcome::Cancelled => {
                fail(
                    messages,
                    terminal,
                    req.event_sink,
                    req.session_id,
                    "Turno cancelado.",
                    TerminalState::Cancelled,
                )
                .await
            }
        }
    }
}

enum OneShotOutcome {
    Response(nexum_agent::agent::Reasoning),
    Failed(String),
    TimedOut,
    Cancelled,
}

async fn fail(
    messages: Vec<BaseMessage>,
    terminal: TerminalController,
    sink: Arc<dyn EventSink>,
    session_id: String,
    message: &str,
    state: TerminalState,
) -> PromptResult {
    terminal
        .finish(sink.as_ref(), &session_id, state, Some(message))
        .await;
    PromptResult {
        messages,
        ok: false,
        stop_reason: if state == TerminalState::Cancelled {
            PromptStopReason::Cancelled
        } else {
            PromptStopReason::EndTurn
        },
        recall_items: Vec::new(),
    }
}

#[cfg(test)]
#[path = "one_shot_test.rs"]
mod tests;
