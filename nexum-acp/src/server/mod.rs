//! ACP Server — transport-agnostic request handler.
//!
//! Accepts any [`AcpTransport`] implementation (mpsc for TUI, stdio for IDE),
//! builds and executes ReAct agents, and pushes [`SessionUpdate`] notifications
//! back through the transport.
//!
//! **Cancel architecture**: `session/prompt` execution is spawned into a
//! background tokio task so the main server loop remains responsive to
//! `session/cancel` notifications. Sessions are shared via
//! `Arc<tokio::sync::Mutex<HashMap>>`.

use std::{collections::HashMap, sync::Arc};

pub use nexum_acp::session::state_builders::{
    apply_thinking_effort, build_config_options, build_mode_state, parse_permission_mode,
};
use nexum_acp::transport::types::IncomingMessage;
use nexum_agent::{
    agent::AgentCancellationToken, interaction::ChannelState, messages::BaseMessage,
};
use nexum_middlewares::prelude::*;

use crate::provider::{LlmProvider, NexumConfig};
use crate::runtime::{RuntimeHealthState, RuntimeMetadata};

mod notify;
mod prompt;
mod requests;

pub(crate) use notify::{extract_session_id, handle_notification, send_session_info_update};
pub(crate) use prompt::execute_prompt;
pub(crate) use requests::handle_request;

/// Decide si el servidor inicia la predicción opcional posterior a un prompt.
/// La UI aporta su adaptador y un host sin interfaz usa [`NoopPredictionPolicy`].
pub trait PredictionPolicy: Send + Sync {
    fn is_enabled(&self) -> bool;
}

/// Política segura para hosts sin interfaz de usuario.
pub struct NoopPredictionPolicy;

impl PredictionPolicy for NoopPredictionPolicy {
    fn is_enabled(&self) -> bool {
        false
    }
}

// ── Session state ────────────────────────────────────────────────────────────

pub(crate) struct SessionState {
    #[allow(dead_code)] // session 标识字段，保留供调试
    session_id: String,
    thread_id: String,
    cwd: String,
    history: Vec<BaseMessage>,
    cancel_token: Option<AgentCancellationToken>,
    // ── Frozen session data (populated at creation, immutable thereafter) ──
    pub(crate) frozen: Option<nexum_acp::session::executor::FrozenSessionData>,
    /// Recall items from previous turn (injected as <system-reminder> in next user message).
    pub(crate) recall_items: Vec<String>,
    /// Session-scoped agent component pool for reusing heavy objects across prompts.
    pub(crate) agent_pool: nexum_acp::session::agent_pool::AgentPool,
}

// ── Server config ────────────────────────────────────────────────────────────

/// All cross-session configuration needed by the ACP server.
pub struct AcpServerConfig {
    pub provider: Arc<parking_lot::RwLock<LlmProvider>>,
    pub nexum_config: Arc<parking_lot::RwLock<NexumConfig>>,
    pub permission_mode: Arc<SharedPermissionMode>,
    pub cron_control: CronControlClient,
    pub mcp_pool: Option<Arc<nexum_middlewares::mcp::McpClientPool>>,
    pub channel_state: Option<Arc<ChannelState>>,
    pub plugin_skill_roots: Vec<nexum_middlewares::skills::SkillRoot>,
    pub plugin_agent_dirs: Vec<std::path::PathBuf>,
    pub plugin_hooks: Vec<nexum_middlewares::hooks::RegisteredHook>,
    pub hook_groups: Vec<Vec<nexum_middlewares::hooks::RegisteredHook>>,
    pub plugin_lsp_servers: Vec<nexum_lsp::config::LspServerConfig>,
    pub tool_search_index: Arc<nexum_middlewares::tool_search::ToolSearchIndex>,
    pub shared_tools:
        Arc<parking_lot::RwLock<HashMap<String, Arc<dyn nexum_agent::tools::BaseTool>>>>,
    pub thread_store: Arc<dyn nexum_agent::thread::ThreadStore>,
    pub langfuse_session: Option<Arc<nexum_acp::langfuse::LangfuseSession>>,
    pub config_path: std::path::PathBuf,
    /// 共享 SessionManager：用于支撑 cascade cancel 子 agent 与 goal_state。
    ///
    /// TUI 本地仍维护 SessionState（history/frozen/agent_pool 等），但 SubAgent
    /// 注册/注销与 goal_state 通过 SessionManager 中的 AcpSession 记录管理，
    /// 保证 `execute_prompt` 接收 `Some(session_manager)` 时 cascade cancel 生效。
    pub session_manager: nexum_acp::session::SessionManager,
    pub prediction_policy: Arc<dyn PredictionPolicy>,
    /// Optional host-owned broker for durable cron interactions. The ACP server
    /// never assumes SQLite or a particular authorization model.
    pub pending_interaction_broker: Option<Arc<dyn nexum_acp::cron::PendingInteractionBroker>>,
    /// Runtime-owned metadata exposed by the host-neutral runtime RPCs.
    pub runtime: RuntimeMetadata,
}

// ── Main server loop ────────────────────────────────────────────────────────

type SharedSessions = Arc<tokio::sync::Mutex<HashMap<String, SessionState>>>;

/// Main ACP server loop. Accepts any `AcpTransport` (mpsc for TUI, stdio for IDE).
///
/// `session/prompt` is spawned into a background task so the loop stays
/// responsive to `session/cancel` and other incoming messages.
pub async fn run_acp_server(
    transport: Arc<dyn nexum_acp::transport::AcpTransport>,
    cfg: AcpServerConfig,
) {
    cfg.runtime.health.set(RuntimeHealthState::Ready);
    let sessions: SharedSessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    // Per-session prompt serialization lock: ensures that when a prompt completes
    // (state.history updated) the next prompt for the same session sees the updated history.
    let prompt_locks: Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    while let Some(msg) = transport.recv().await {
        match msg {
            IncomingMessage::Request {
                id,
                method,
                params,
                caller,
            } => {
                if method == "session/prompt" {
                    if cfg.runtime.health.get() != RuntimeHealthState::Ready {
                        let _ = transport
                            .send_response(
                                id,
                                Err(crate::transport::types::AcpError::new(
                                    -32000,
                                    "runtime is not ready to accept prompts",
                                )),
                            )
                            .await;
                        continue;
                    }
                    // Spawn long-running prompt execution so the server loop
                    // continues processing session/cancel notifications.
                    let sessions = sessions.clone();
                    let transport = Arc::clone(&transport);
                    let provider = cfg.provider.clone();
                    let nexum_config = cfg.nexum_config.clone();
                    let permission_mode = cfg.permission_mode.clone();
                    let cron_control = cfg.cron_control.clone();
                    let plugin_skill_roots = cfg.plugin_skill_roots.clone();
                    let plugin_agent_dirs = cfg.plugin_agent_dirs.clone();
                    let hook_groups = cfg.hook_groups.clone();
                    let mcp_pool = cfg.mcp_pool.clone();
                    let channel_state = cfg.channel_state.clone();
                    let tool_search_index = cfg.tool_search_index.clone();
                    let shared_tools = cfg.shared_tools.clone();
                    let plugin_lsp_servers = cfg.plugin_lsp_servers.clone();
                    let thread_store = cfg.thread_store.clone();
                    let prompt_session_id = extract_session_id(&params, "").to_string();
                    let langfuse_session = cfg.langfuse_session.clone();
                    let session_manager = cfg.session_manager.clone();
                    let prediction_policy = cfg.prediction_policy.clone();

                    // Extract AgentPool from session, wrap in Arc<Mutex> for
                    // in-place modification inside executor.
                    let pool_arc = {
                        let mut sessions = sessions.lock().await;
                        let pool = sessions
                            .get_mut(&prompt_session_id)
                            .map(|s| std::mem::take(&mut s.agent_pool))
                            .unwrap_or_default();
                        Arc::new(parking_lot::Mutex::new(pool))
                    };

                    let prompt_lock = {
                        let mut locks = prompt_locks.lock().await;
                        locks
                            .entry(prompt_session_id.clone())
                            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                            .clone()
                    };

                    tokio::spawn(async move {
                        // Serialize prompts per session: wait for any in-flight prompt to finish
                        // so that state.history is up-to-date when this prompt reads it.
                        let _guard = prompt_lock.lock().await;
                        let result = execute_prompt(
                            params,
                            &sessions,
                            &provider,
                            &nexum_config,
                            &permission_mode,
                            cron_control,
                            &plugin_skill_roots,
                            &plugin_agent_dirs,
                            &hook_groups,
                            mcp_pool,
                            channel_state,
                            tool_search_index,
                            shared_tools,
                            &plugin_lsp_servers,
                            &transport,
                            &thread_store,
                            langfuse_session,
                            pool_arc.clone(),
                            session_manager,
                        )
                        .await;

                        // Prediction: agent 成功完成后发起预测输入请求。
                        // Gate Nexum (sprint ghost-text 2026-07-07): sólo si el
                        // usuario habilitó NEXUM_PREDICTIONS=on. Default off →
                        // ni siquiera se dispara el LLM fork (ahorra la llamada
                        // y evita el ghost text alucinado en el composer).
                        if result.is_ok() && prediction_policy.is_enabled() {
                            let pred_transport = Arc::clone(&transport);
                            let pred_session_id = prompt_session_id.clone();
                            let pred_provider = provider.clone();
                            let pred_sessions = sessions.clone();

                            tokio::spawn(async move {
                                tracing::debug!("Prediction task started");
                                // 从 session 获取最新历史
                                let (history, cwd) = {
                                    let sessions = pred_sessions.lock().await;
                                    match sessions.get(&pred_session_id) {
                                        Some(s) => (s.history.clone(), s.cwd.clone()),
                                        None => {
                                            tracing::debug!("Prediction: session not found");
                                            return;
                                        }
                                    }
                                };

                                // 取最近 10 条消息作为上下文（排除 System 消息）
                                let recent: Vec<_> = history
                                    .iter()
                                    .rev()
                                    .filter(|m| !m.is_system())
                                    .take(10)
                                    .cloned()
                                    .collect();
                                let recent: Vec<_> = recent.into_iter().rev().collect();

                                if recent.is_empty() {
                                    tracing::debug!("Prediction: no recent messages");
                                    return;
                                }
                                tracing::debug!(count = recent.len(), "Prediction: got messages");

                                // 直接复用已构建的 LlmProvider（绕过 from_config）
                                let llm_provider = pred_provider.read().clone();
                                tracing::debug!("Prediction: LLM provider ready");

                                // Facade：agent 构建与执行统一由 nexum-acp executor 承担，
                                // TUI 层不再直接构建 ReActAgent（遵守 CLAUDE.md [TRAP]）。
                                let result = nexum_acp::session::executor::execute_prediction(
                                    llm_provider,
                                    recent,
                                    &cwd,
                                )
                                .await;

                                match result {
                                    Ok(text) => {
                                        if !text.is_empty() {
                                            tracing::debug!(%text, "Prediction ready, sending notification");
                                            let _ = pred_transport
                                                .send_notification(
                                                    "peri/prediction_ready",
                                                    serde_json::json!({
                                                        "sessionId": pred_session_id,
                                                        "text": text,
                                                    }),
                                                )
                                                .await;
                                        } else {
                                            tracing::debug!("Prediction: LLM returned empty text");
                                        }
                                    }
                                    Err(nexum_acp::session::executor::PredictionError::Failed(
                                        e,
                                    )) => {
                                        tracing::debug!(error = %e, "Prediction fork failed");
                                    }
                                    Err(nexum_acp::session::executor::PredictionError::Timeout) => {
                                        tracing::debug!("Prediction fork timed out (30s)");
                                    }
                                }
                            });
                        }

                        // Restore AgentPool back into session
                        if let Ok(mutex) = Arc::try_unwrap(pool_arc) {
                            let mut sessions = sessions.lock().await;
                            if let Some(state) = sessions.get_mut(&prompt_session_id) {
                                state.agent_pool = mutex.into_inner();
                            }
                        }

                        let _ = transport.send_response(id, result).await;
                        if !prompt_session_id.is_empty() {
                            send_session_info_update(transport.as_ref(), &prompt_session_id).await;
                        }
                    });
                } else {
                    let mut sessions = sessions.lock().await;
                    let result = handle_request(
                        &method,
                        &params,
                        caller.as_ref(),
                        &cfg,
                        &mut sessions,
                        transport.as_ref(),
                    )
                    .await;
                    let _ = transport.send_response(id, result).await;
                }
            }
            IncomingMessage::Notification { method, params } => {
                let sessions = sessions.lock().await;
                handle_notification(&method, &params, &sessions, &cfg);
            }
            IncomingMessage::Response { .. } => {
                // Responses are routed internally by the transport's pending map.
            }
        }
    }
    cfg.runtime.health.set(RuntimeHealthState::ShuttingDown);
}
