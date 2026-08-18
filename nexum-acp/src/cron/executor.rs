use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::{bail, Result};
use async_trait::async_trait;
use chrono::Utc;
use nexum_agent::{
    agent::AgentCancellationToken,
    interaction::{
        ApprovalDecision, InteractionContext, InteractionResponse, QuestionAnswer,
        UserInteractionBroker,
    },
    messages::BaseMessage,
};

use super::{CronJob, CronRun, InteractionPolicy, PendingInteractionSink, PendingInteractionSpec};

/// Output persisted for a completed cron occurrence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CronRunOutput {
    pub result: Option<String>,
}

/// The only non-success terminal state a controlled headless execution can
/// produce. It is never retried and resolving its interaction cannot restart
/// the original agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CronRunOutcome {
    Succeeded(CronRunOutput),
    FailedNeedsUser { reason: String },
}

impl CronRunOutput {
    pub fn text(result: impl Into<String>) -> Self {
        Self {
            result: Some(result.into()),
        }
    }
}

/// Executes one claimed cron occurrence. The runtime deliberately has no agent,
/// provider or thread-store implementation of its own.
#[async_trait]
pub trait CronRunExecutor: Send + Sync {
    async fn execute(&self, job: CronJob, run: CronRun) -> Result<CronRunOutcome>;
}

/// Builds the normal ACP prompt context for a scheduled job. Host adapters must
/// target the existing `job.target_thread_id` session and set `job.prompt` as the
/// prompt content; they must not create a parallel session implementation.
#[async_trait]
pub trait CronPromptContextFactory: Send + Sync {
    async fn build(&self, job: &CronJob, run: &CronRun) -> Result<CronPromptExecutionContext>;
}

/// Shared execution context plus the signal written only after the interaction
/// sink durably records the headless interaction.
pub struct CronPromptExecutionContext {
    pub prompt: crate::session::executor::PromptExecutionContext,
    pub interaction_recorded: Arc<AtomicBool>,
}

/// Adapter that uses the existing ACP executor exactly once per claimed run.
/// It intentionally adds no agent loop, model provider or persistence layer.
pub struct ExecutePromptRunner {
    context_factory: Arc<dyn CronPromptContextFactory>,
}

impl ExecutePromptRunner {
    pub fn new(context_factory: Arc<dyn CronPromptContextFactory>) -> Self {
        Self { context_factory }
    }
}

#[async_trait]
impl CronRunExecutor for ExecutePromptRunner {
    async fn execute(&self, job: CronJob, run: CronRun) -> Result<CronRunOutcome> {
        let context = self.context_factory.build(&job, &run).await?;
        let history_len = context.prompt.history.len();
        let thread_store = context.prompt.thread_store.clone();
        let thread_id = context.prompt.thread_id.clone();
        let result = crate::session::executor::execute_prompt(context.prompt).await;
        if context.interaction_recorded.load(Ordering::Acquire) {
            return Ok(CronRunOutcome::FailedNeedsUser {
                reason: "la ejecución cron requiere una interacción del usuario".to_string(),
            });
        }
        if result.ok {
            if let (Some(store), Some(thread_id)) = (thread_store, thread_id) {
                let thread_id = nexum_agent::thread::ThreadId::from(thread_id);
                let new_messages = result.messages.get(history_len..).ok_or_else(|| {
                    anyhow::anyhow!("el cron compactó el historial; no es seguro persistirlo")
                })?;
                store.append_messages(&thread_id, new_messages).await?;
            }
            Ok(CronRunOutcome::Succeeded(CronRunOutput {
                result: last_assistant_text(&result.messages),
            }))
        } else {
            bail!("execute_prompt terminó sin completar el run cron")
        }
    }
}

fn last_assistant_text(messages: &[BaseMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        BaseMessage::Ai { .. } => {
            let content = message.content();
            (!content.trim().is_empty()).then_some(content)
        }
        _ => None,
    })
}

/// Fails closed when a scheduled execution reaches an interaction point while
/// no ACP client is attached. It never grants a tool permission or invents an
/// answer on the user's behalf.
pub struct HeadlessFailSafeBroker {
    policy: InteractionPolicy,
    sink: Arc<dyn PendingInteractionSink>,
    job: CronJob,
    run: CronRun,
    cancel: AgentCancellationToken,
    interaction_recorded: Arc<AtomicBool>,
}

impl HeadlessFailSafeBroker {
    pub fn new(
        policy: InteractionPolicy,
        sink: Arc<dyn PendingInteractionSink>,
        job: CronJob,
        run: CronRun,
        cancel: AgentCancellationToken,
        interaction_recorded: Arc<AtomicBool>,
    ) -> Self {
        Self {
            policy,
            sink,
            job,
            run,
            cancel,
            interaction_recorded,
        }
    }

    async fn fail_safely(&self, context: InteractionContext) {
        let now = Utc::now();
        let interaction = PendingInteractionSpec {
            run_id: self.run.id.clone(),
            job_id: self.job.id.clone(),
            target_thread_id: self.run.target_thread_id.clone(),
            context,
            expires_at: self.policy.expires_at(now),
        };
        if let Err(error) = self.sink.persist_pending(interaction).await {
            tracing::error!(error = %error, run_id = %self.run.id, "failed to persist headless cron interaction");
        } else {
            self.interaction_recorded.store(true, Ordering::Release);
        }
        // Stop the agent even if recording failed: a headless execution must
        // not continue after it reaches a user-interaction boundary.
        self.cancel.cancel();
    }
}

#[async_trait]
impl UserInteractionBroker for HeadlessFailSafeBroker {
    async fn request(&self, context: InteractionContext) -> InteractionResponse {
        self.fail_safely(context.clone()).await;
        match context {
            InteractionContext::Approval { items } => InteractionResponse::Decisions(
                items
                    .into_iter()
                    .map(|_| ApprovalDecision::Reject {
                        reason: "ejecución cron sin cliente para aprobar la operación".to_string(),
                        source: Some("headless-cron".to_string()),
                    })
                    .collect(),
            ),
            InteractionContext::Questions { requests } => InteractionResponse::Answers(
                requests
                    .into_iter()
                    .map(|question| QuestionAnswer {
                        id: question.id,
                        selected: Vec::new(),
                        text: None,
                    })
                    .collect(),
            ),
        }
    }
}
