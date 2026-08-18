use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use nexum_agent::{
    agent::AgentCancellationToken,
    interaction::UserInteractionBroker,
    messages::MessageContent,
    thread::{ThreadId, ThreadStore},
    tools::BaseTool,
};
use nexum_middlewares::prelude::SharedPermissionMode;

use crate::{
    langfuse::LangfuseSession,
    provider::{LlmProvider, NexumConfig},
    session::{
        agent_pool::AgentPool, event_sink::EventSink, executor::PromptExecutionContext,
        SessionManager,
    },
};

use super::{
    CronJob, CronPromptContextFactory, CronPromptExecutionContext, CronRun, HeadlessFailSafeBroker,
    InteractionPolicy, PendingInteractionSink,
};

/// Host-owned resources required to build the shared prompt execution context.
/// The cron runtime does not own or reimplement any of these services.
#[derive(Clone)]
pub struct CronPromptResources {
    pub provider: Arc<parking_lot::RwLock<LlmProvider>>,
    pub nexum_config: Arc<parking_lot::RwLock<NexumConfig>>,
    pub permission_mode: Arc<SharedPermissionMode>,
    pub cron_control: nexum_middlewares::cron::CronControlClient,
    pub mcp_pool: Option<Arc<nexum_middlewares::mcp::McpClientPool>>,
    pub channel_state: Option<Arc<nexum_agent::interaction::ChannelState>>,
    pub plugin_skill_roots: Vec<nexum_middlewares::skills::SkillRoot>,
    pub plugin_agent_dirs: Vec<PathBuf>,
    pub hook_groups: Vec<Vec<nexum_middlewares::hooks::RegisteredHook>>,
    pub plugin_lsp_servers: Vec<nexum_lsp::config::LspServerConfig>,
    pub tool_search_index: Arc<nexum_middlewares::tool_search::ToolSearchIndex>,
    pub shared_tools: Arc<parking_lot::RwLock<HashMap<String, Arc<dyn BaseTool>>>>,
    pub thread_store: Arc<dyn ThreadStore>,
    pub langfuse_session: Option<Arc<LangfuseSession>>,
    pub session_manager: SessionManager,
    pub interaction_policy: InteractionPolicy,
    pub interaction_sink: Arc<dyn PendingInteractionSink>,
}

/// Builds a normal [`PromptExecutionContext`] from a durable target thread for
/// a headless cron occurrence. Agent construction and execution stay entirely
/// inside `execute_prompt`.
pub struct HeadlessPromptContextFactory {
    resources: CronPromptResources,
}

impl HeadlessPromptContextFactory {
    pub fn new(resources: CronPromptResources) -> Self {
        Self { resources }
    }
}

#[async_trait]
impl CronPromptContextFactory for HeadlessPromptContextFactory {
    async fn build(&self, job: &CronJob, run: &CronRun) -> Result<CronPromptExecutionContext> {
        let thread_id = ThreadId::from(job.target_thread_id.clone());
        let meta = self
            .resources
            .thread_store
            .load_meta(&thread_id)
            .await
            .with_context(|| format!("cargar thread cron {}", job.target_thread_id))?;
        let history = self
            .resources
            .thread_store
            .load_context(&thread_id)
            .await
            .with_context(|| format!("cargar contexto cron {}", job.target_thread_id))?;
        self.resources
            .session_manager
            .ensure_session(&job.target_thread_id, &meta.cwd);
        let frozen = self.resources.session_manager.build_frozen_data(
            &meta.cwd,
            &self.resources.plugin_skill_roots,
            &self.resources.plugin_agent_dirs,
        );

        let event_sink: Arc<dyn EventSink> = Arc::new(HeadlessEventSink);
        let cancel = AgentCancellationToken::new();
        let interaction_recorded = Arc::new(AtomicBool::new(false));
        let broker: Arc<dyn UserInteractionBroker> = Arc::new(HeadlessFailSafeBroker::new(
            self.resources.interaction_policy,
            self.resources.interaction_sink.clone(),
            job.clone(),
            run.clone(),
            cancel.clone(),
            interaction_recorded.clone(),
        ));
        Ok(CronPromptExecutionContext {
            prompt: PromptExecutionContext {
                provider: self.resources.provider.read().clone(),
                nexum_config: Arc::new(self.resources.nexum_config.read().clone()),
                cwd: meta.cwd,
                session_id: job.target_thread_id.clone(),
                cancel,
                event_sink,
                broker,
                permission_mode: self.resources.permission_mode.clone(),
                content: MessageContent::text(job.prompt.clone()),
                stable_profile: false,
                task_envelope: None,
                frozen: Some(frozen),
                history,
                incoming_recalls: Vec::new(),
                session_start_source: None,
                bg_results: Vec::new(),
                plugin_skill_roots: self.resources.plugin_skill_roots.clone(),
                plugin_agent_dirs: self.resources.plugin_agent_dirs.clone(),
                hook_groups: self.resources.hook_groups.clone(),
                cron_control: self.resources.cron_control.clone(),
                mcp_pool: self.resources.mcp_pool.clone(),
                channel_state: self.resources.channel_state.clone(),
                tool_search_index: self.resources.tool_search_index.clone(),
                shared_tools: self.resources.shared_tools.clone(),
                lsp_servers: self.resources.plugin_lsp_servers.clone(),
                langfuse_session: self.resources.langfuse_session.clone(),
                pool: Arc::new(parking_lot::Mutex::new(AgentPool::new())),
                thread_store: Some(self.resources.thread_store.clone()),
                thread_id: Some(job.target_thread_id.clone()),
                session_manager: Some(self.resources.session_manager.clone()),
            },
            interaction_recorded,
        })
    }
}

/// Drops execution events because scheduled jobs have no attached client.
struct HeadlessEventSink;

#[async_trait]
impl EventSink for HeadlessEventSink {
    async fn push_event(
        &self,
        _session_id: &str,
        _event: &nexum_agent::agent::events::AgentEvent,
        _context_window: u32,
    ) {
    }

    async fn push_done(&self, _session_id: &str) {}
}
