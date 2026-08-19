//! Shared prompt execution logic.
//!
//! Provides [`execute_prompt`] which encapsulates the common agent execution
//! pipeline used by both TUI (via [`TransportEventSink`]) and stdio (via
//! [`StdioEventSink`]) paths.
//!
//! Compact 由 CompactMiddleware（before_model 钩子）在 ReAct 循环内原地处理，
//! 不再需要外层 loop + resubmit。

use std::sync::Arc;

use nexum_agent::{
    agent::{
        events::{AgentEvent as ExecutorEvent, AgentEventHandler},
        state::AgentState,
        token::ContextBudget,
        AgentCancellationToken, State,
    },
    error::AgentError,
    interaction::{ChannelState, UserInteractionBroker},
    messages::{BaseMessage, ContentBlock, MessageContent, MessageId},
};
use tokio::sync::oneshot;
use tracing::{debug, error};

use crate::{
    agent::builder::{self, AcpAgentConfig},
    langfuse::{LangfuseSession, LangfuseTracer},
    prompt::{build_system_prompt, PromptFeatures},
    provider::LlmProvider,
    session::{
        agent_pool::{AgentPool, CachedLlmInstances},
        agent_runtime::{AgentRuntime, CancelPolicy},
        event_sink::EventSink,
        SessionManager,
    },
};

/// High-level reason why prompt execution stopped, used to derive ACP `StopReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptStopReason {
    /// Normal completion — the agent finished its turn.
    EndTurn,
    /// The user cancelled via `session/cancel`.
    Cancelled,
    /// The agent reached the maximum number of iterations.
    MaxTurnRequests,
}

/// Result of prompt execution.
pub struct PromptResult {
    /// Updated message history after execution.
    pub messages: Vec<BaseMessage>,
    /// Whether execution succeeded.
    pub ok: bool,
    /// Why the prompt execution stopped.
    pub stop_reason: PromptStopReason,
    /// Recall items collected during execution (for next turn injection).
    pub recall_items: Vec<String>,
}

/// Session-scoped frozen data that locks system prompt stability.
///
/// Populated at session creation time by `session/new`, passed through to
/// every turn's agent build to guarantee the system prompt never changes
/// within a session.
///
/// # Immutable Value Object
///
/// 唯一构造路径是 [`FrozenSessionData::build`]；字段对外不可变，只通过
/// accessor 方法读取。String 字段以 `Arc<str>` 存储，clone 零成本——
/// frozen 数据每轮被 `AcpAgentConfig` clone 一次，共享底层数据避免拷贝。
/// 会话内一旦构造完成即不再可变（系统提示词稳定性第一优先级）。
#[derive(Clone)]
pub struct FrozenSessionData {
    /// Full system prompt string built at session creation.
    system_prompt: Arc<str>,
    /// Frozen content of CLAUDE.md (with resolved `@import`), None if no file.
    claude_md: Option<Arc<str>>,
    /// Frozen content of CLAUDE.local.md, None if no file.
    claude_local_md: Option<Arc<str>>,
    /// Frozen skills summary string, None if no skills.
    skill_summary: Option<Arc<str>>,
    /// Session creation date in YYYY-MM-DD format.
    date: Arc<str>,
    /// Whether cwd was a git repo at session creation time.
    is_git_repo: bool,
    /// Session creation language preference (e.g. "zh-CN", "en").
    /// None = auto-detect from user input (no explicit instruction).
    language: Option<Arc<str>>,
}

impl FrozenSessionData {
    /// 唯一构造入口：在 `session/new` 时调用，捕获 cwd/language/CLAUDE.md/
    /// skills/system_prompt/date。
    ///
    /// 吸收原 `crate::session::frozen::build_frozen_session_data` 的全部逻辑，
    /// 保证任何构造路径都经过此方法（Immutable Value Object 构造约束）。
    /// `language` 为 `None` 时由 LLM 自行从用户输入 auto-detect。
    pub fn build(
        cwd: &str,
        language: Option<&str>,
        plugin_skill_roots: &[nexum_middlewares::skills::SkillRoot],
        plugin_agent_dirs: &[std::path::PathBuf],
        frozen_date: &str,
    ) -> Self {
        let (claude_md, claude_local_md) =
            nexum_middlewares::AgentsMdMiddleware::read_frozen_content(cwd);

        // 一次性读取 disableBundledSkills 并冻结到 frozen_skill_summary
        // （保持系统提示词稳定性：会话内不重读）
        let disable_bundled = nexum_middlewares::skills::load_disable_bundled_skills();
        let skill_summary = nexum_middlewares::SkillsMiddleware::build_frozen_summary(
            cwd,
            plugin_skill_roots.to_vec(),
            disable_bundled,
        );

        let features = crate::prompt::PromptFeatures::detect();
        let system_prompt = crate::prompt::build_system_prompt(
            None,
            cwd,
            features,
            plugin_agent_dirs,
            Some(frozen_date),
            language,
        );

        let is_git_repo = std::path::Path::new(cwd).join(".git").exists();

        Self {
            system_prompt: Arc::from(system_prompt),
            claude_md: claude_md.map(Arc::from),
            claude_local_md: claude_local_md.map(Arc::from),
            skill_summary: skill_summary.map(Arc::from),
            date: Arc::from(frozen_date),
            is_git_repo,
            language: language.map(Arc::from),
        }
    }

    /// 会话内冻结的完整 system prompt 字符串。
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// 冻结的 CLAUDE.md 内容（已解析 `@import`），无文件时为 None。
    pub fn claude_md(&self) -> Option<&str> {
        self.claude_md.as_deref()
    }

    /// 冻结的 CLAUDE.local.md 内容，无文件时为 None。
    pub fn claude_local_md(&self) -> Option<&str> {
        self.claude_local_md.as_deref()
    }

    /// 冻结的 skills summary 字符串，无 skills 时为 None。
    pub fn skill_summary(&self) -> Option<&str> {
        self.skill_summary.as_deref()
    }

    /// 会话创建日期（YYYY-MM-DD 格式）。
    pub fn date(&self) -> &str {
        &self.date
    }

    /// 会话创建时 cwd 是否为 git 仓库。
    pub fn is_git_repo(&self) -> bool {
        self.is_git_repo
    }

    /// 会话创建时的语言偏好（如 "zh-CN"、"en"）。None 表示 auto-detect。
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }
}

/// Parameter Object for [`execute_prompt`].
///
/// Groups 30 positional parameters into a single struct to eliminate
/// `#[allow(clippy::too_many_arguments)]` and reduce call-site placeholder
/// noise. Construction uses named-field syntax; default values are explicit
/// at each call site (no builder hiding required state).
///
/// # Fields by concern
/// - **Session-level identity & transport**：`provider` / `nexum_config` / `cwd`
///   / `session_id` / `cancel` / `event_sink` / `broker` / `permission_mode`
/// - **Per-turn content**：`content` / `frozen` / `history` / `incoming_recalls`
///   / `session_start_source` / `bg_results`
/// - **Middleware chain resources**：`plugin_skill_roots` / `plugin_agent_dirs`
///   / `hook_groups` / `cron_control` / `mcp_pool` / `channel_state`
///   / `tool_search_index` / `shared_tools` / `lsp_servers` / `langfuse_session`
/// - **Session-scoped caches & persistence**：`pool` / `thread_store` / `thread_id`
///   / `session_manager`
pub struct PromptExecutionContext {
    // ── Session-level identity & transport ───────────────────────────────────
    /// 当前激活的 LLM provider（snapshot，每轮从 `Arc<RwLock<>>` 克隆）。
    pub provider: LlmProvider,
    /// 全局 peri 配置（snapshot，每轮从 `Arc<RwLock<>>` 克隆）。
    pub nexum_config: Arc<crate::provider::NexumConfig>,
    /// 会话工作目录。
    pub cwd: String,
    /// 会话 ID（用于事件路由、SessionManager 查询、Langfuse trace）。
    pub session_id: String,
    /// 取消令牌（由 SessionManager 管理，clone 后传入 executor）。
    pub cancel: AgentCancellationToken,
    /// 事件出口（TUI 用 TransportEventSink，stdio 用 StdioEventSink）。
    pub event_sink: Arc<dyn EventSink>,
    /// 用户交互 broker（HITL/AskUser 通道）。
    pub broker: Arc<dyn UserInteractionBroker>,
    /// 权限模式共享句柄。
    pub permission_mode: Arc<nexum_middlewares::prelude::SharedPermissionMode>,

    // ── Per-turn content ──────────────────────────────────────────────────────
    /// 用户本轮输入。
    pub content: MessageContent,
    /// Explicit marker set by the stable TUI client. Missing envelopes on this
    /// route fail closed instead of falling through to the legacy agent path.
    pub stable_profile: bool,
    /// Optional V1 task contract supplied by the ACP request. It carries no
    /// conversation history and is not used to construct tools or providers.
    pub task_envelope: Option<crate::task::TaskEnvelopeV1>,
    /// 会话级 frozen 数据（system prompt 稳定性锚点）。
    pub frozen: Option<FrozenSessionData>,
    /// 现有历史消息（执行前）。
    pub history: Vec<BaseMessage>,
    /// 上一轮 recall 注入项。
    pub incoming_recalls: Vec<String>,
    /// SessionStart matcher：startup / resume / clear / compact。
    /// None 表示不触发 SessionStart。
    pub session_start_source: Option<String>,
    /// 后台任务结果（注入合成的 AgentResult tool_use/tool_result）。
    pub bg_results: Vec<nexum_agent::agent::events::BackgroundTaskResult>,

    // ── Middleware chain resources ────────────────────────────────────────────
    /// 插件 skill 根列表（携带 source/plugin_name）。
    pub plugin_skill_roots: Vec<nexum_middlewares::skills::SkillRoot>,
    /// 插件 agent 目录列表。
    pub plugin_agent_dirs: Vec<std::path::PathBuf>,
    /// Hook 组（按全局/项目/本地分层）。
    pub hook_groups: Vec<Vec<nexum_middlewares::hooks::RegisteredHook>>,
    /// Host-owned cron control endpoint. Missing host is represented by a
    /// diagnostic client, never by an in-process scheduler.
    pub cron_control: nexum_middlewares::cron::CronControlClient,
    /// MCP client 池。
    pub mcp_pool: Option<Arc<nexum_middlewares::mcp::McpClientPool>>,
    /// Channel broker 共享状态（AskUser 走 channel 时使用）。
    pub channel_state: Option<Arc<ChannelState>>,
    /// 工具搜索索引（Deferred Tools 发现）。
    pub tool_search_index: Arc<nexum_middlewares::tool_search::ToolSearchIndex>,
    /// 共享工具表（运行时动态注册的工具）。
    pub shared_tools: Arc<
        parking_lot::RwLock<
            std::collections::HashMap<String, Arc<dyn nexum_agent::tools::BaseTool>>,
        >,
    >,
    /// LSP server 配置。
    pub lsp_servers: Vec<nexum_lsp::config::LspServerConfig>,
    /// Langfuse 会话级句柄（None 表示禁用遥测）。
    pub langfuse_session: Option<Arc<LangfuseSession>>,

    // ── Session-scoped caches & persistence ───────────────────────────────────
    /// AgentPool（LLM/Compact model 缓存，session 级）。
    pub pool: Arc<parking_lot::Mutex<AgentPool>>,
    /// 持久化存储（None 表示 print 模式不持久化）。
    pub thread_store: Option<Arc<dyn nexum_agent::thread::ThreadStore>>,
    /// 当前 thread ID（持久化 + SubAgent 注册）。
    pub thread_id: Option<String>,
    /// SessionManager（用于 cascade cancel 子 agent + register/deregister runtime）。
    pub session_manager: Option<SessionManager>,
}

/// Per-turn computed configuration derived from `PromptExecutionContext`.
///
/// Built once at the top of [`execute_prompt`], passed by reference to
/// [`build_and_execute_agent`] to avoid recomputing and to keep the agent
/// builder function signature manageable.
struct TurnConfig<'a> {
    provider: &'a LlmProvider,
    nexum_config: &'a Arc<crate::provider::NexumConfig>,
    cwd: &'a str,
    frozen: Option<&'a FrozenSessionData>,
    language: Option<String>,
    cancel: &'a AgentCancellationToken,
    permission_mode: &'a Arc<nexum_middlewares::prelude::SharedPermissionMode>,
    broker: &'a Arc<dyn UserInteractionBroker>,
    session_start_source: Option<String>,
    compact_config: nexum_agent::agent::compact::CompactConfig,
    disable_compact: bool,
    budget: ContextBudget,
    auxiliary_model: Option<Arc<dyn nexum_agent::llm::BaseModel>>,
    effective_context_window: u32,
}

/// Shared agent execution pipeline with auto-compact support.
///
/// This is the orchestrator. The actual work is split across four private
/// helpers:
/// - [`intercept_immediate_command`]：slash 命令拦截（Immediate 直接返回，不构建 agent）
/// - [`spawn_event_pump`]：后台事件泵 + Langfuse tracer
/// - [`build_and_execute_agent`]：agent 构建 + 执行 + 状态收集
/// - [`collect_result`]：close channel + 等待 pump drain + recall 提取
///
/// The caller is responsible for:
/// - Session management (storing/retrieving cwd, history, cancel_token)
/// - Choosing the broker (HITL/AskUser handler)
/// - Providing the correct `EventSink` implementation
pub async fn execute_prompt(ctx: PromptExecutionContext) -> PromptResult {
    // 解构 ctx：所有字段一次性 move，避免后续部分 move 导致的借用冲突。
    // 注意：history/content/bg_results 在 move 前先用引用读取（compact_config 等不需要 move）。
    let PromptExecutionContext {
        provider,
        nexum_config,
        cwd,
        session_id,
        cancel,
        event_sink,
        broker,
        permission_mode,
        content,
        stable_profile,
        task_envelope,
        frozen,
        history,
        incoming_recalls,
        session_start_source,
        bg_results,
        plugin_skill_roots,
        plugin_agent_dirs,
        hook_groups,
        cron_control,
        mcp_pool,
        channel_state,
        tool_search_index,
        shared_tools,
        lsp_servers,
        langfuse_session,
        pool,
        thread_store,
        thread_id,
        session_manager,
    } = ctx;

    if stable_profile {
        use crate::runtime::stable_turn::StableTurnPolicy;
        use crate::session::{
            one_shot::{OneShotExecutor, OneShotRequest},
            terminal::{TerminalController, TerminalState},
        };

        let policy = StableTurnPolicy::stable();
        let required_envelope = match policy.require_envelope(task_envelope.as_ref()) {
            Ok(envelope) => envelope,
            Err(error) => {
                let terminal = TerminalController::new(cancel.clone());
                terminal
                    .finish(
                        event_sink.as_ref(),
                        &session_id,
                        TerminalState::Failed,
                        Some(&error.to_string()),
                    )
                    .await;
                return PromptResult {
                    messages: history,
                    ok: false,
                    stop_reason: PromptStopReason::EndTurn,
                    recall_items: incoming_recalls,
                };
            }
        };
        let flow = match Some(required_envelope) {
            Some(envelope) if bg_results.is_empty() => {
                match policy.validate_envelope(envelope, &provider) {
                    Ok(flow) => flow,
                    Err(error) => {
                        let terminal = TerminalController::new(cancel.clone());
                        terminal
                            .finish(
                                event_sink.as_ref(),
                                &session_id,
                                TerminalState::Failed,
                                Some(&error.to_string()),
                            )
                            .await;
                        return PromptResult {
                            messages: history,
                            ok: false,
                            stop_reason: PromptStopReason::EndTurn,
                            recall_items: incoming_recalls,
                        };
                    }
                }
            }
            Some(_) => {
                let terminal = TerminalController::new(cancel.clone());
                terminal
                    .finish(
                        event_sink.as_ref(),
                        &session_id,
                        TerminalState::Failed,
                        Some("automatic continuations are forbidden in the stable profile"),
                    )
                    .await;
                return PromptResult {
                    messages: history,
                    ok: false,
                    stop_reason: PromptStopReason::EndTurn,
                    recall_items: incoming_recalls,
                };
            }
            None => unreachable!("required envelope was validated above"),
        };
        let context_window = provider.context_window();
        let provider_name = provider.display_name().to_string();
        let model_name = provider.model_name().to_string();
        return OneShotExecutor::new(policy)
            .execute(
                flow,
                OneShotRequest {
                    model: provider.into_model(),
                    provider_name,
                    model_name,
                    content,
                    history,
                    session_id,
                    system_prompt: "Respondé directamente al usuario. No solicites ni uses herramientas, MCP, subagentes o continuaciones.".to_string(),
                    context_window,
                    cancel,
                    event_sink,
                },
            )
            .await;
    }

    // Un envelope explícito restringe los schemas expuestos al modelo. La
    // ausencia conserva el conjunto de herramientas de los clientes ACP legados.
    let explicit_micro = task_envelope
        .as_ref()
        .is_some_and(is_local_micro_e2e_envelope);
    // NEXUM_LOCAL_FAST_DEMO_V1 — DIRECT_CHAT: decisión determinística previa al
    // turno (sin LLM). Default ON (`NEXUM_LOCAL_FAST=0` lo apaga) y sólo en turnos
    // interactivos SIN envelope explícito, para no pisar la semántica de ningún
    // TaskEnvelopeV1. Un prompt simple ("Respondé únicamente X", saludo, pregunta
    // trivial) se rutea al mismo camino mínimo del envelope E2E: system prompt
    // reducido (identidad/locale/objetivo/seguridad, sin secciones pesadas) y
    // CERO tools. Preserva TaskEnvelopeV1 → Hormiguero → ACP (todo aguas arriba).
    // `is_some()` NO alcanza: la ruta estable de la TUI fabrica un envelope para
    // cada mensaje, así que con ese criterio todo turno de chat tomaba el tope
    // de 500. Se pregunta por el ORIGEN, con allowlist.
    let had_explicit_envelope =
        crate::agent::builder::admite_tope_largo(task_envelope.as_ref());
    let direct_chat = crate::flow::should_direct_chat(
        crate::flow::local_fast_enabled(),
        task_envelope.is_some(),
        &content.text_content(),
    );
    let local_micro = explicit_micro || direct_chat;
    let allowed_tools = if direct_chat {
        // Cero tools: DIRECT_CHAT es conversación pura, un solo roundtrip.
        Some(Vec::new())
    } else {
        task_envelope.map(|envelope| envelope.allowed_tools)
    };
    // Los DOS caminos dejan traza. Antes sólo la dejaba DIRECT_CHAT, así que
    // el único flujo que no decía por dónde iba era justamente el que se
    // colgaba: una pantalla muda y ningún registro de qué eligió el
    // clasificador.
    // Persistir el ruteo: `tracing` vive en memoria y se pierde; esto queda en
    // ~/.nexum/metrics/ para que el próximo cuelgue sea legible después.
    crate::turn_log::log_turn_routed(
        if direct_chat { "DIRECT_CHAT" } else { "FULL_REACT" },
        crate::flow::local_fast_enabled(),
        had_explicit_envelope,
        // DIRECT_CHAT fija 0 tools; FULL_REACT usa las del envelope, que acá
        // todavía no están resueltas: se registra -1 = "sin límite fijado".
        if direct_chat { 0 } else { usize::MAX },
    );
    if direct_chat {
        tracing::info!(
            flow = "DIRECT_CHAT",
            tools = 0,
            "turno simple ruteado a contexto mínimo sin tools"
        );
    } else {
        tracing::info!(
            flow = "FULL_REACT",
            explicit_envelope = had_explicit_envelope,
            local_fast = crate::flow::local_fast_enabled(),
            "turno ruteado al loop con tools"
        );
    }

    // Un modelo que el catálogo declara SIN tool calling no va a producir una
    // llamada por más vueltas que dé. Antes eso se descubría gastando el tope
    // entero; ahora se dice de entrada. Sólo bloquea el estado `Unsupported`:
    // no saber no es lo mismo que no poder, así que un modelo sin dato se
    // intenta igual.
    if !direct_chat {
        let modelo = provider.model_name().to_string();
        if crate::provider::model_tool_support_any(&modelo)
            == crate::provider::ToolSupport::Unsupported
        {
            crate::turn_log::log_turn_end("FULL_REACT", "model_without_tool_support", 0);
            tracing::warn!(
                model = %modelo,
                "turno con herramientas rechazado: el catálogo declara que el modelo no las soporta"
            );
            event_sink
                .push_event(
                    &session_id,
                    &ExecutorEvent::AgentExecutionFailed {
                        message: mensaje_modelo_sin_tools(&modelo),
                    },
                    0,
                )
                .await;
            // Sin `push_done` la TUI queda en loading para siempre: fallar
            // temprano y dejar la pantalla colgada sería peor que las quince
            // vueltas que este chequeo evita.
            event_sink.push_done(&session_id).await;
            // El mensaje va TAMBIÉN en la historia, no sólo por el sink: en
            // `--print` no hay TUI que consuma eventos, y devolver el turno sin
            // texto sería una pantalla muda — exactamente lo que este chequeo
            // existe para evitar.
            let mut messages = history;
            messages.push(BaseMessage::ai(mensaje_modelo_sin_tools(&modelo)));
            return PromptResult {
                messages,
                ok: false,
                stop_reason: PromptStopReason::EndTurn,
                recall_items: incoming_recalls,
            };
        }
    }

    // bg_results 注入合成的 AgentResult tool_use/tool_result 消息
    let (history, content) = if !bg_results.is_empty() {
        inject_bg_result_messages(history, content, &bg_results)
    } else {
        (history, content)
    };

    // Compact config — computed early for command interception and agent building.
    let mut compact_config = nexum_config.config.compact.clone().unwrap_or_default();
    compact_config.apply_env_overrides();
    let disable_compact = std::env::var("DISABLE_COMPACT").is_ok()
        || std::env::var("DISABLE_AUTO_COMPACT").is_ok()
        || !compact_config.auto_compact_enabled;

    // Auxiliary model — reuse AgentPool cache if available, otherwise create fresh.
    // 共享于 CompactMiddleware（摘要）与 Goal 工具（完成度验证）。
    let cached_llm = {
        let pool_guard = pool.lock();
        if pool_guard.has_valid_cache(&provider) {
            pool_guard.get_cached_llm().cloned()
        } else {
            None
        }
    };
    let auxiliary_model: Option<Arc<dyn nexum_agent::llm::BaseModel>> = if disable_compact {
        None
    } else {
        cached_llm
            .as_ref()
            .map(|c| c.auxiliary_model.clone())
            .or_else(|| Some(provider.clone().into_model().into()))
    };

    // Context window (前置计算，供 bg event pump 和 compact 使用)
    let context_window = provider.context_window();
    let context_1m = nexum_config.config.context_1m.unwrap_or(false);
    let effective_context_window = if context_1m {
        1_000_000
    } else {
        context_window
    };

    // 前置创建 bg 通道（BgCommand 等 Immediate 命令依赖）
    let (bg_event_tx_for_cmd, mut bg_event_rx_for_cmd) =
        tokio::sync::mpsc::unbounded_channel::<ExecutorEvent>();
    let (bg_notification_tx_for_cmd, _bg_notification_rx_for_cmd) =
        tokio::sync::mpsc::unbounded_channel();
    let bg_registry_for_cmd = Arc::new(nexum_middlewares::subagent::BackgroundTaskRegistry::new(
        bg_notification_tx_for_cmd,
    ));

    // BgCommand 事件的 bg event pump（必须在命令拦截之前启动，Immediate 命令才能发事件）
    {
        let bg_cmd_sink = Arc::clone(&event_sink);
        let bg_cmd_sid = session_id.clone();
        let bg_cmd_cw = effective_context_window;
        tokio::spawn(async move {
            while let Some(bg_event) = bg_event_rx_for_cmd.recv().await {
                bg_cmd_sink
                    .push_event(&bg_cmd_sid, &bg_event, bg_cmd_cw)
                    .await;
            }
        });
    }

    // Command interception — check if content is a slash command before building agent.
    if let Some(immediate) = intercept_immediate_command(InterceptRequest {
        content: &content,
        history: &history,
        cwd: &cwd,
        session_id: &session_id,
        cancel: &cancel,
        nexum_config: &nexum_config,
        event_sink: &event_sink,
        auxiliary_model: &auxiliary_model,
        thread_store: thread_store.clone(),
        thread_id: thread_id.clone(),
        bg_event_tx: &bg_event_tx_for_cmd,
        bg_registry: &bg_registry_for_cmd,
        frozen: frozen.as_ref(),
    })
    .await
    {
        return immediate;
    }

    let trace_input = content.text_content();
    let agent_input = if incoming_recalls.is_empty() {
        nexum_agent::agent::react::AgentInput::blocks(content)
    } else {
        let reminder_text = format!(
            "<system-reminder>\n{}\n</system-reminder>",
            incoming_recalls.join("\n")
        );
        let mut blocks = content.content_blocks();
        blocks.push(ContentBlock::text(reminder_text));
        nexum_agent::agent::react::AgentInput::blocks(MessageContent::blocks(blocks))
    };

    // Context budget (computed once, uses compact_config from above)
    let budget = ContextBudget::new(effective_context_window)
        .with_auto_compact_threshold(compact_config.auto_compact_threshold)
        .with_warning_threshold(compact_config.micro_compact_threshold);

    // Event channel (lives for entire execute_prompt lifetime)
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<ExecutorEvent>();
    let event_tx = Arc::new(parking_lot::Mutex::new(Some(event_tx)));

    // 将会 move 进 BuildAgentRequest 的 middleware resources（无法借用，必须 move）。
    // turn 仍以引用形式借用 provider/nexum_config/cwd/cancel/permission_mode/broker。
    let turn = TurnConfig {
        provider: &provider,
        nexum_config: &nexum_config,
        cwd: &cwd,
        frozen: frozen.as_ref(),
        language: frozen
            .as_ref()
            .and_then(|f| f.language().map(|s| s.to_string()))
            .or_else(|| nexum_config.config.language.clone()),
        cancel: &cancel,
        permission_mode: &permission_mode,
        broker: &broker,
        session_start_source,
        compact_config: compact_config.clone(),
        disable_compact,
        budget,
        auxiliary_model: auxiliary_model.clone(),
        effective_context_window,
    };

    // Main event pump
    let pump_handle = spawn_event_pump(SpawnPumpRequest {
        event_rx,
        sink: Arc::clone(&event_sink),
        session_id: session_id.clone(),
        effective_context_window,
        langfuse_session: langfuse_session.clone(),
        trace_input: trace_input.to_string(),
        provider_display_name: provider.display_name().to_string(),
    });

    // 把会 move 的资源打包成 struct，turn + event_tx + cached_llm 仍借用。
    // 由于 prompt builder 需要的所有资源都在这里 move 进 BuildAgentRequest，
    // 调用方后续不再访问这些字段（session_id 在 collect_result 借用，
    // 此时 BuildAgentRequest 已 drop）。
    let exec_outcome = build_and_execute_agent(BuildAgentRequest {
        turn: &turn,
        agent_input,
        history,
        plugin_skill_roots,
        plugin_agent_dirs,
        hook_groups,
        cron_control,
        mcp_pool,
        channel_state,
        tool_search_index,
        shared_tools,
        lsp_servers,
        langfuse_session,
        pool,
        thread_store,
        thread_id,
        session_manager,
        event_sink: &event_sink,
        session_id: &session_id,
        event_tx: &event_tx,
        cached_llm: cached_llm.as_ref(),
        allowed_tools,
        has_explicit_envelope: had_explicit_envelope,
        local_micro,
    })
    .await;

    collect_result(CollectRequest {
        event_tx: &event_tx,
        pump_handle,
        session_id: &session_id,
        exec_outcome,
    })
    .await
}

// ── Intercept Request parameter object ─────────────────────────────────────

/// 命令拦截请求（参数对象，避免 12 个位置参数）。
struct InterceptRequest<'a> {
    content: &'a MessageContent,
    history: &'a [BaseMessage],
    cwd: &'a str,
    session_id: &'a str,
    cancel: &'a AgentCancellationToken,
    nexum_config: &'a Arc<crate::provider::NexumConfig>,
    event_sink: &'a Arc<dyn EventSink>,
    auxiliary_model: &'a Option<Arc<dyn nexum_agent::llm::BaseModel>>,
    thread_store: Option<Arc<dyn nexum_agent::thread::ThreadStore>>,
    thread_id: Option<String>,
    bg_event_tx: &'a tokio::sync::mpsc::UnboundedSender<ExecutorEvent>,
    bg_registry: &'a Arc<nexum_middlewares::subagent::BackgroundTaskRegistry>,
    frozen: Option<&'a FrozenSessionData>,
}

/// 命令拦截：检查 content 是否为 Immediate 类型 slash 命令。
///
/// 返回 `Some(PromptResult)` 表示已处理（agent 不构建）；
/// 返回 `None` 表示继续走 agent 管线。
///
/// [TRAP] Immediate 命令路径绕过 agent event pump，必须手动调用 `sink.push_done()`。
/// 否则 TUI 界面永久卡在 loading 状态（issue_2026-05-29-immediate-command-missing-push-done）。
async fn intercept_immediate_command(req: InterceptRequest<'_>) -> Option<PromptResult> {
    let text = req.content.text_content();
    let stripped = text.strip_prefix('/')?;
    if stripped.is_empty() {
        return None;
    }

    let command_registry = crate::session::command::default_command_registry();
    let (cmd, args) = command_registry.find(&text)?;
    if cmd.kind() != crate::session::command::CommandKind::Immediate {
        // Passthrough/Transform → fall through to normal agent flow
        return None;
    }

    tracing::debug!(
        command = %cmd.name(),
        history_len = req.history.len(),
        "Immediate command intercepted"
    );
    let ctx = crate::session::command::CommandContext {
        session_id: req.session_id.to_string(),
        history: req.history.to_vec(),
        cwd: req.cwd.to_string(),
        nexum_config: Arc::new(req.nexum_config.as_ref().clone()),
        auxiliary_model: req.auxiliary_model.clone(),
        event_sink: req.event_sink.clone(),
        args: args.to_string(),
        cancel_token: req.cancel.clone(),
        thread_store: req.thread_store,
        thread_id: req.thread_id,
        bg_event_sender: Some(req.bg_event_tx.clone()),
        bg_registry: Some(req.bg_registry.clone()),
        frozen_claude_md: req
            .frozen
            .as_ref()
            .and_then(|f| f.claude_md().map(|s| Arc::new(s.to_string()))),
        frozen_claude_local_md: req
            .frozen
            .as_ref()
            .and_then(|f| f.claude_local_md().map(|s| Arc::new(s.to_string()))),
        frozen_skill_summary: req
            .frozen
            .as_ref()
            .and_then(|f| f.skill_summary().map(|s| Arc::new(s.to_string()))),
    };
    let result = tokio::select! {
        r = cmd.execute(ctx) => r,
        _ = req.cancel.cancelled() => {
            tracing::info!(session_id = %req.session_id, "Immediate command cancelled");
            crate::session::command::CommandResult {
                messages: req.history.to_vec(),
                stop_reason: PromptStopReason::Cancelled,
            }
        }
    };
    // Immediate 命令跳过 agent event pump，必须手动发送 push_done
    // 通知 TUI agent 执行完成，否则界面永久卡在 loading 状态。
    req.event_sink.push_done(req.session_id).await;
    Some(PromptResult {
        messages: result.messages,
        ok: true,
        stop_reason: result.stop_reason,
        recall_items: Vec::new(),
    })
}

// ── Spawn Pump Request parameter object ─────────────────────────────────────

/// 事件泵启动请求（参数对象）。
struct SpawnPumpRequest {
    event_rx: tokio::sync::mpsc::UnboundedReceiver<ExecutorEvent>,
    sink: Arc<dyn EventSink>,
    session_id: String,
    effective_context_window: u32,
    langfuse_session: Option<Arc<LangfuseSession>>,
    trace_input: String,
    provider_display_name: String,
}

/// 后台事件泵句柄，通过 oneshot channel 与 pump_done_rx 配对。
struct PumpHandle {
    pump_done_rx: oneshot::Receiver<()>,
}

/// 启动主事件泵任务。
///
/// 任务循环：
/// 1. trace_start → recv events → forward to sink
/// 2. trace_end + push_done → signal pump completion（在 Langfuse flush 之前）
/// 3. Langfuse flush（fire-and-forget，不得阻塞管线）
fn spawn_event_pump(req: SpawnPumpRequest) -> PumpHandle {
    let SpawnPumpRequest {
        mut event_rx,
        sink,
        session_id,
        effective_context_window,
        langfuse_session,
        trace_input,
        provider_display_name,
    } = req;

    let (pump_done_tx, pump_done_rx) = oneshot::channel();

    let langfuse_tracer = langfuse_session
        .as_ref()
        .map(|s| parking_lot::Mutex::new(LangfuseTracer::new(Arc::clone(s), session_id.clone())));
    if langfuse_tracer.is_some() {
        debug!(session_id = %session_id, "Langfuse tracer created for turn");
    }

    tokio::spawn(async move {
        // Start Langfuse trace
        if let Some(ref tracer) = langfuse_tracer {
            tracer.lock().on_trace_start(&trace_input);
        }

        while let Some(exec_event) = event_rx.recv().await {
            // Langfuse tracing
            if let Some(ref tracer) = langfuse_tracer {
                forward_langfuse_event(tracer, &exec_event, &provider_display_name);
            }

            sink.push_event(&session_id, &exec_event, effective_context_window)
                .await;
        }

        // End Langfuse trace and flush
        let langfuse_flush = if let Some(tracer) = langfuse_tracer {
            let handle = tracer.into_inner().on_trace_end(None);
            Some(handle)
        } else {
            None
        };

        sink.push_done(&session_id).await;

        // Signal pump completion BEFORE Langfuse flush.
        // Langfuse is telemetry — it must never block the execution pipeline.
        // Without this, a slow/unreachable Langfuse API blocks pump_done_tx,
        // which blocks wait_for_pump(), which blocks execute_prompt() from
        // returning, which holds the prompt_lock and prevents the next prompt
        // from starting. Ctrl+C can't recover because the new prompt's cancel
        // token hasn't been created yet (still waiting on the lock).
        let _ = pump_done_tx.send(());

        // Langfuse flush: fire-and-forget. The spawned task runs independently;
        // worst-case it blocks for ~150s (HTTP 30s × 3 retries + backoff) then
        // logs warnings. The pump has already signaled completion above, so this
        // never blocks the execution pipeline.
        drop(langfuse_flush);
    });

    PumpHandle { pump_done_rx }
}

/// 转发单个 executor 事件到 Langfuse tracer（pump 内的纯函数，便于测试）。
fn forward_langfuse_event(
    tracer: &parking_lot::Mutex<LangfuseTracer>,
    exec_event: &ExecutorEvent,
    provider_display_name: &str,
) {
    match exec_event {
        ExecutorEvent::LlmCallStart {
            step,
            messages,
            tools,
        } => {
            tracer.lock().on_llm_start(*step, messages, tools);
        }
        ExecutorEvent::LlmCallEnd {
            step,
            model,
            output,
            usage,
            stop_reason: _,
        } => {
            tracer
                .lock()
                .on_llm_end(*step, model, provider_display_name, output, usage.as_ref());
        }
        ExecutorEvent::ToolStart {
            tool_call_id,
            name,
            input,
            ..
        } => {
            tracer.lock().on_tool_start(tool_call_id, name, input);
        }
        ExecutorEvent::ToolEnd {
            tool_call_id,
            output,
            is_error,
            ..
        } => {
            tracer.lock().on_tool_end(tool_call_id, output, *is_error);
        }
        ExecutorEvent::TextChunk { chunk, .. } => {
            tracer.lock().on_text_chunk(chunk);
        }
        ExecutorEvent::LlmRetrying {
            attempt,
            max_attempts,
            delay_ms,
            error,
        } => {
            tracer
                .lock()
                .on_llm_retrying(*attempt, *max_attempts, *delay_ms, error);
        }
        ExecutorEvent::CompactStarted => {
            tracer.lock().on_compact_start();
        }
        ExecutorEvent::CompactCompleted {
            summary,
            files,
            skills,
            micro_cleared,
            ..
        } => {
            tracer.lock().on_compact_end(
                summary,
                files.len(),
                skills.len(),
                *micro_cleared,
                false,
                "",
            );
        }
        ExecutorEvent::CompactError { message } => {
            tracer.lock().on_compact_end("", 0, 0, 0, true, message);
        }
        _ => {}
    }
}

// ── Build Agent Request parameter object ────────────────────────────────────

/// Agent 构建请求（参数对象）。
///
/// `turn` 携带本轮计算出的紧凑配置（provider/config/compact 等），
/// 其余字段是中间件链所需的所有共享资源。
struct BuildAgentRequest<'a> {
    turn: &'a TurnConfig<'a>,
    agent_input: nexum_agent::agent::react::AgentInput,
    history: Vec<BaseMessage>,
    // ── 会 move 的中间件资源 ────────────────────────────────────────────────
    plugin_skill_roots: Vec<nexum_middlewares::skills::SkillRoot>,
    plugin_agent_dirs: Vec<std::path::PathBuf>,
    hook_groups: Vec<Vec<nexum_middlewares::hooks::RegisteredHook>>,
    cron_control: nexum_middlewares::cron::CronControlClient,
    mcp_pool: Option<Arc<nexum_middlewares::mcp::McpClientPool>>,
    channel_state: Option<Arc<ChannelState>>,
    tool_search_index: Arc<nexum_middlewares::tool_search::ToolSearchIndex>,
    shared_tools: Arc<
        parking_lot::RwLock<
            std::collections::HashMap<String, Arc<dyn nexum_agent::tools::BaseTool>>,
        >,
    >,
    lsp_servers: Vec<nexum_lsp::config::LspServerConfig>,
    langfuse_session: Option<Arc<LangfuseSession>>,
    pool: Arc<parking_lot::Mutex<AgentPool>>,
    thread_store: Option<Arc<dyn nexum_agent::thread::ThreadStore>>,
    thread_id: Option<String>,
    session_manager: Option<SessionManager>,
    // ── 借用的引用 ──────────────────────────────────────────────────────────
    event_sink: &'a Arc<dyn EventSink>,
    session_id: &'a str,
    event_tx:
        &'a Arc<parking_lot::Mutex<Option<tokio::sync::mpsc::UnboundedSender<ExecutorEvent>>>>,
    cached_llm: Option<&'a CachedLlmInstances>,
    allowed_tools: Option<Vec<String>>,
    has_explicit_envelope: bool,
    local_micro: bool,
}

/// Agent 执行后的最终输出（state + 停止原因）。
struct ExecOutcome {
    ok: bool,
    stop_reason: PromptStopReason,
    agent_state: AgentState,
}

/// 构建 + 执行 agent。包含：
/// - system prompt 解析（frozen 或 legacy 重建）
/// - SubAgentMiddleware register/deregister 闭包
/// - `build_agent` 调用 + AgentPool 缓存回写
/// - bg event pump + todo 转发 pump 启动
/// - agent.execute + 错误事件转发
/// - cancel cascade 子 agent
async fn build_and_execute_agent(req: BuildAgentRequest<'_>) -> ExecOutcome {
    let BuildAgentRequest {
        turn,
        agent_input,
        history,
        plugin_skill_roots,
        plugin_agent_dirs,
        hook_groups,
        cron_control,
        mcp_pool,
        channel_state,
        tool_search_index,
        shared_tools,
        lsp_servers,
        langfuse_session: _langfuse_session,
        pool,
        thread_store,
        thread_id,
        session_manager,
        event_sink,
        session_id,
        event_tx,
        cached_llm,
        allowed_tools,
        has_explicit_envelope,
        local_micro,
    } = req;

    let (
        system_prompt,
        frozen_claude_md,
        frozen_claude_local_md,
        frozen_skill_summary,
        frozen_date,
    ) = if local_micro {
        (
            build_system_prompt(
                None,
                turn.cwd,
                PromptFeatures::local_micro(),
                &[],
                None,
                turn.language.as_deref(),
            ),
            None,
            None,
            None,
            None,
        )
    } else if let Some(f) = turn.frozen {
        // 使用 session 创建时冻结的数据，跳过重建
        (
            f.system_prompt().to_string(),
            f.claude_md().map(|s| s.to_string()),
            f.claude_local_md().map(|s| s.to_string()),
            f.skill_summary().map(|s| s.to_string()),
            Some(f.date().to_string()),
        )
    } else {
        // Legacy 路径：未提供 frozen 数据时每轮重建 system prompt。
        //
        // [TRAP] 当前仅 print mode (`-p`, cli_print.rs:207 `frozen: None`) 进入此分支，
        // 单轮执行后退出，因此 "per-turn rebuild" 实际不会发生。
        // SubAgent 不走此路径——它们的 system prompt 由 builder.rs:356-366 的
        // system_builder closure 独立构造。
        //
        // 加 warn! 提升可观测性：如果未来有新调用方忘记传 frozen 数据，
        // 日志会立刻暴露（违反 frozen 不变量 = 第一优先级）。
        tracing::warn!(
            cwd = %turn.cwd,
            "execute_prompt: frozen data 未提供，回退到 per-turn rebuild 路径（仅 print mode 合法）"
        );
        let features = PromptFeatures::detect();
        let sp = build_system_prompt(
            None,
            turn.cwd,
            features,
            &plugin_agent_dirs,
            None,
            turn.language.as_deref(),
        );
        (sp, None, None, None, None)
    };

    // Build register/deregister closures for SubAgentMiddleware
    let register_runtime = session_manager.clone().map(|sm| {
        let sid = session_id.to_string();
        Arc::new(
            move |thread_id: String, cancel_token: AgentCancellationToken, policy: String| {
                if let Some(mut session) = sm.get_session_mut(&sid) {
                    let runtime =
                        AgentRuntime::new(thread_id.clone(), CancelPolicy::from_str(&policy));
                    // Store the provided cancel_token so external cancellation works
                    let rt = AgentRuntime {
                        thread_id,
                        cancel_token,
                        cancel_policy: runtime.cancel_policy,
                        status: runtime.status,
                    };
                    session.active_agents.insert(rt.thread_id.clone(), rt);
                }
            },
        ) as crate::agent::builder::RegisterRuntimeFn
    });
    let deregister_runtime = session_manager.clone().map(|sm| {
        let sid = session_id.to_string();
        Arc::new(move |thread_id: &str| {
            if let Some(mut session) = sm.get_session_mut(&sid) {
                session.active_agents.remove(thread_id);
            }
        }) as crate::agent::builder::DeregisterRuntimeFn
    });

    let event_handler: Arc<dyn AgentEventHandler> =
        Arc::new(nexum_agent::agent::events::FnEventHandler({
            let tx = event_tx.clone();
            move |event: ExecutorEvent| {
                if let Some(tx) = tx.lock().as_ref() {
                    let _ = tx.send(event);
                }
            }
        }));

    // 从 session_manager 获取 goal_state（实现 GoalController trait）
    let goal_controller: Option<Arc<dyn nexum_agent::goal::GoalController>> = session_manager
        .as_ref()
        .and_then(|sm| sm.goal_state_for(session_id))
        .map(|gs| Arc::new(gs) as Arc<dyn nexum_agent::goal::GoalController>);

    let (agent_output, new_cache) = builder::build_agent(
        AcpAgentConfig {
            provider: turn.provider.clone(),
            cwd: turn.cwd.to_string(),
            system_prompt,
            frozen: builder::FrozenData {
                claude_md: frozen_claude_md,
                claude_local_md: frozen_claude_local_md,
                skill_summary: frozen_skill_summary,
                date: frozen_date,
            },
            event_handler,
            cancel: turn.cancel.clone(),
            permission_mode: turn.permission_mode.clone(),
            nexum_config: Arc::new(turn.nexum_config.as_ref().clone()),
            cron_control,
            agent_overrides: None,
            preload_skills: Vec::new(),
            session_id: Some(session_id.to_string()),
            broker: turn.broker.clone(),
            plugin_skill_roots,
            plugin_agent_dirs,
            hook_groups,
            session_start_source: turn.session_start_source.clone(),
            mcp_pool,
            channel_state,
            tool_search_index,
            shared_tools,
            allowed_tools,
            has_explicit_envelope,
            child_handler_factory: None,
            lsp_servers,
            compact: builder::CompactSettings {
                config: if turn.disable_compact {
                    None
                } else {
                    Some(turn.compact_config.clone())
                },
                budget: if turn.disable_compact {
                    None
                } else {
                    Some(turn.budget.clone())
                },
                model: turn.auxiliary_model.clone(),
                event_tx: Some(event_tx.clone()),
            },
            thread_persistence: builder::ThreadPersistence {
                store: thread_store,
                parent_thread_id: thread_id,
                register_runtime,
                deregister_runtime,
            },
            goal_controller,
        },
        cached_llm,
        &pool,
    );

    // Store updated cache back into pool
    if let Some(cache) = new_cache {
        pool.lock().store_llm(cache);
    }

    // Phase 2: bg event pump — starts before executor runs so events arrive
    // promptly even for tasks completing mid-execution. Outlives executor;
    // exits when all bg spawn closures finish and drop their senders.
    {
        let mut bg_event_rx = agent_output.bg_event_rx;
        let bg_session_id = session_id.to_string();
        let bg_sink = Arc::clone(event_sink);
        let bg_cw = turn.effective_context_window;
        tokio::spawn(async move {
            let mut bg_event_count: u64 = 0;
            while let Some(bg_event) = bg_event_rx.recv().await {
                bg_event_count += 1;
                if matches!(&bg_event, ExecutorEvent::BackgroundTaskCompleted(_)) {
                    tracing::info!(
                        count = bg_event_count,
                        "[bg-diag] bg-event-pump: received BackgroundTaskCompleted"
                    );
                }
                bg_sink.push_event(&bg_session_id, &bg_event, bg_cw).await;
            }
            tracing::info!(
                total_bg_events = bg_event_count,
                "[bg-diag] bg-event-pump: channel closed, exiting"
            );
        });
    }

    // 转发 todo 更新为 ExecutorEvent::TodoUpdate
    let mut todo_rx = agent_output.todo_rx;
    let tx_for_todo = event_tx.clone();
    tokio::spawn(async move {
        while let Some(todos) = todo_rx.recv().await {
            let entries: Vec<nexum_agent::agent::events::TodoEntry> = todos
                .into_iter()
                .map(|t| nexum_agent::agent::events::TodoEntry {
                    content: t.content,
                    active_form: t.active_form,
                    status: match t.status {
                        nexum_middlewares::tools::todo::TodoStatus::Pending => {
                            nexum_agent::agent::events::TodoStatus::Pending
                        }
                        nexum_middlewares::tools::todo::TodoStatus::InProgress => {
                            nexum_agent::agent::events::TodoStatus::InProgress
                        }
                        nexum_middlewares::tools::todo::TodoStatus::Completed => {
                            nexum_agent::agent::events::TodoStatus::Completed
                        }
                    },
                })
                .collect();
            if let Some(tx) = tx_for_todo.lock().as_ref() {
                let _ = tx.send(ExecutorEvent::TodoUpdate(entries));
            }
        }
    });

    // Execute agent
    // Qué sale hacia el proveedor, ANTES de mandarlo. `history` es la lista
    // completa del turno: si crece con repetidos, se quema contexto y se
    // confunde al modelo, y eso no se ve mirando la pantalla.
    {
        use std::collections::HashSet;
        use std::hash::{Hash, Hasher};
        let mut vistos: HashSet<u64> = HashSet::new();
        let mut duplicados = 0usize;
        let mut chars = 0usize;
        let mut acumulador = std::collections::hash_map::DefaultHasher::new();
        for m in &history {
            let repr = format!("{m:?}");
            chars += repr.len();
            repr.hash(&mut acumulador);
            let mut h = std::collections::hash_map::DefaultHasher::new();
            repr.hash(&mut h);
            if !vistos.insert(h.finish()) {
                duplicados += 1;
            }
        }
        nexum_agent::turn_log::log_turn_request(
            history.len(),
            duplicados,
            chars,
            &format!("{:016x}", acumulador.finish()),
        );
    }
    let mut agent_state = AgentState::with_messages(turn.cwd.to_string(), history);
    agent_state.set_context("session_id", session_id);
    agent_state.set_context("run_id", uuid::Uuid::now_v7().to_string());
    let result = agent_output
        .executor
        .execute(agent_input, &mut agent_state, Some(turn.cancel.clone()))
        .await;
    drop(agent_output.executor);

    let ok = result.is_ok();
    if let Err(e) = &result {
        error!(session_id = %session_id, error = %e, "Agent execution failed");
        // El tope agotado no es un error opaco: es un turno que hizo cosas y no
        // llegó a cerrar. Se le cuenta al usuario en su idioma, con lo que sí
        // consiguió, en vez de mandarle el Display del error.
        let message = match e {
            AgentError::MaxIterationsExceeded(limite) => {
                mensaje_tope_agotado(*limite, agent_state.messages())
            }
            otro => otro.to_string(),
        };
        if let Some(tx) = event_tx.lock().as_ref() {
            let _ = tx.send(ExecutorEvent::AgentExecutionFailed { message });
        }
    }

    let stop_reason = if turn.cancel.is_cancelled() {
        PromptStopReason::Cancelled
    } else if matches!(&result, Err(AgentError::MaxIterationsExceeded(_))) {
        PromptStopReason::MaxTurnRequests
    } else if matches!(&result, Err(AgentError::Interrupted)) {
        PromptStopReason::Cancelled
    } else {
        PromptStopReason::EndTurn
    };

    // Cancel cascade children when this agent is cancelled
    if stop_reason == PromptStopReason::Cancelled {
        if let Some(ref sm) = session_manager {
            if let Some(session) = sm.get_session(session_id) {
                session.cancel_cascade_children();
            }
        }
    }

    ExecOutcome {
        ok,
        stop_reason,
        agent_state,
    }
}

/// Mensaje para el turno que pide herramientas contra un modelo que el catálogo
/// declara incapaz de usarlas.
///
/// Se emite ANTES de gastar una sola vuelta. La alternativa —dejarlo intentar—
/// son quince iteraciones que terminan en el mismo lugar, con la diferencia de
/// que el usuario esperó para llegar ahí.
pub(crate) fn mensaje_modelo_sin_tools(modelo: &str) -> String {
    format!(
        "El modelo `{modelo}` no sabe usar herramientas, así que este pedido no \
         puede resolverse con él.\n\n\
         El dato sale del catálogo: el proveedor declara que este modelo no \
         soporta tool calling. No es que haya fallado — no lo intenta.\n\n\
         Elegí otro modelo con `/modelo`, o preguntame algo que se responda sin \
         herramientas."
    )
}

/// Arma el mensaje que ve el usuario cuando el loop de ReAct agota el tope de
/// iteraciones.
///
/// Un tope sin mensaje es el mismo cuelgue, sólo que más corto: la pantalla
/// queda muda igual. Antes de esto el usuario recibía `Max iterations exceeded
/// (500)` — en inglés, sin decir qué pasó ni qué se llegó a hacer.
///
/// Lo que el turno SÍ consiguió (las herramientas que corrió, el último texto
/// que alcanzó a producir) vive en el estado del agente, así que el mensaje se
/// arma acá, que es donde ese estado está completo.
pub(crate) fn mensaje_tope_agotado(
    limite: usize,
    mensajes: &[nexum_agent::messages::BaseMessage],
) -> String {
    // Herramientas efectivamente pedidas, en orden de primera aparición y con
    // su conteo. Interesa qué hizo, no la lista cruda de 500 llamadas.
    let mut orden: Vec<String> = Vec::new();
    let mut veces: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for m in mensajes {
        for tc in m.tool_calls() {
            let n = veces.entry(tc.name.clone()).or_insert(0);
            if *n == 0 {
                orden.push(tc.name.clone());
            }
            *n += 1;
        }
    }

    // Último texto no vacío del asistente: es la mitad de respuesta que ya
    // había producido y que hoy se tiraba a la basura junto con el error.
    let parcial = mensajes
        .iter()
        .rev()
        .filter(|m| matches!(m, nexum_agent::messages::BaseMessage::Ai { .. }))
        .map(|m| m.content())
        .find(|c| !c.trim().is_empty());

    let mut out = format!(
        "Corté el turno: llegué al tope de {limite} pasos sin terminar la tarea."
    );

    if orden.is_empty() {
        out.push_str("\n\nNo llegué a usar ninguna herramienta.");
    } else {
        let total: usize = veces.values().sum();
        out.push_str(&format!(
            "\n\nLo que sí hice ({total} llamadas a herramientas):"
        ));
        for nombre in orden.iter().take(8) {
            let n = veces.get(nombre).copied().unwrap_or(0);
            if n > 1 {
                out.push_str(&format!("\n  · {nombre} ×{n}"));
            } else {
                out.push_str(&format!("\n  · {nombre}"));
            }
        }
        if orden.len() > 8 {
            out.push_str(&format!("\n  · … y {} herramientas más", orden.len() - 8));
        }
    }

    if let Some(texto) = parcial {
        let texto = texto.trim();
        let corte: String = if texto.chars().count() > 600 {
            texto.chars().take(600).collect::<String>() + "…"
        } else {
            texto.to_string()
        };
        out.push_str("\n\nLo último que alcancé a escribir:\n");
        out.push_str(&corte);
    }

    out.push_str(
        "\n\nPedime que siga desde acá, o acotá el pedido a un paso a la vez.",
    );
    out
}

const LOCAL_MICRO_E2E_SENTINEL: &str = "Respondé únicamente con: NEXUM_ACP_E2E_OK";

fn is_local_micro_e2e_envelope(envelope: &crate::task::TaskEnvelopeV1) -> bool {
    envelope.version == crate::task::TaskEnvelopeVersion::V1
        && envelope.envelope_id == "host-e2e"
        && envelope.source == crate::task::TaskSource::Voice
        && envelope.objective == LOCAL_MICRO_E2E_SENTINEL
        && envelope.user_input == LOCAL_MICRO_E2E_SENTINEL
        && envelope.workspace.as_deref() == Some(".")
        && envelope.constraints.is_empty()
        && envelope.allowed_tools.is_empty()
        && envelope.evidence_refs.is_empty()
        && envelope.success_criteria.is_empty()
        && envelope.output_format == crate::task::OutputFormat::Text
        && envelope.execution_budget == crate::task::ExecutionBudgetV1::default()
        && !envelope.evidence_policy.require_evidence
        && envelope.evidence_policy.minimum_evidence_refs == 0
        && envelope.evidence_policy.allow_unverified_output
        && envelope.priority == crate::task::TaskPriority::Normal
        && envelope.risk == crate::task::TaskRisk::Low
        && envelope.sanitized_metadata.is_empty()
}

// ── Collect Result Request parameter object ─────────────────────────────────

/// 结果收集请求（参数对象）。
struct CollectRequest<'a> {
    event_tx:
        &'a Arc<parking_lot::Mutex<Option<tokio::sync::mpsc::UnboundedSender<ExecutorEvent>>>>,
    pump_handle: PumpHandle,
    session_id: &'a str,
    exec_outcome: ExecOutcome,
}

/// 最终结果收集：close channel → 等待 pump drain → 提取 recall items。
///
/// 顺序约束：必须先 close event_tx，pump 才能退出 recv 循环；然后等待 pump_done。
async fn collect_result(req: CollectRequest<'_>) -> PromptResult {
    let CollectRequest {
        event_tx,
        pump_handle,
        session_id,
        mut exec_outcome,
    } = req;

    close_channel(event_tx);
    wait_for_pump(pump_handle.pump_done_rx, session_id).await;

    let recall_items = exec_outcome.agent_state.drain_recall();
    PromptResult {
        messages: exec_outcome.agent_state.into_messages(),
        ok: exec_outcome.ok,
        stop_reason: exec_outcome.stop_reason,
        recall_items,
    }
}

fn close_channel(
    event_tx: &Arc<parking_lot::Mutex<Option<tokio::sync::mpsc::UnboundedSender<ExecutorEvent>>>>,
) {
    let mut tx_guard = event_tx.lock();
    *tx_guard = None;
}

async fn wait_for_pump(pump_done_rx: oneshot::Receiver<()>, session_id: &str) {
    match tokio::time::timeout(std::time::Duration::from_secs(10), pump_done_rx).await {
        Ok(Ok(())) => debug!(session_id, "Event pump done"),
        Ok(Err(_)) => error!(session_id, "Event pump done channel closed unexpectedly"),
        Err(_) => error!(
            session_id,
            "Event pump timed out (10s) — Langfuse flush may have blocked push_done"
        ),
    }
}

/// Inject synthetic AgentResult tool_use + tool_result messages into the conversation history.
///
/// When background agents complete, the TUI sends a `prompt_with_bg_results` request.
/// This function prepends synthetic messages to the history so the LLM sees them as
/// prior tool calls and results:
///
/// ```text
/// history (prepended):
///   AI: [ToolUse(AgentResult, task_id=xxx), ToolUse(AgentResult, task_id=yyy)]
///   Tool: [tool_result for xxx]
///   Tool: [tool_result for yyy]
/// ```
///
/// Returns modified history with synthetic messages prepended.
fn inject_bg_result_messages(
    mut history: Vec<BaseMessage>,
    user_content: MessageContent,
    bg_results: &[nexum_agent::agent::events::BackgroundTaskResult],
) -> (Vec<BaseMessage>, MessageContent) {
    // Build tool_use blocks (one per bg result) and collect ID mappings
    let mut tool_use_blocks = Vec::new();
    let mut tool_result_data: Vec<(String, &nexum_agent::agent::events::BackgroundTaskResult)> =
        Vec::new();

    for result in bg_results {
        let tool_use_id = MessageId::new().as_uuid().to_string();
        tool_use_blocks.push(ContentBlock::ToolUse {
            id: tool_use_id.clone(),
            name: "AgentResult".to_string(),
            input: serde_json::json!({
                "task_id": result.task_id,
            }),
        });
        tool_result_data.push((tool_use_id, result));
    }

    // 1. AI message with tool_use blocks
    let ai_msg = BaseMessage::ai_from_blocks(tool_use_blocks);
    history.push(ai_msg);

    // 2. One tool_result message per bg result
    for (tool_use_id, result) in tool_result_data {
        let result_text = result.to_notification();
        let tool_msg = if result.success {
            BaseMessage::tool_result(&tool_use_id, result_text)
        } else {
            BaseMessage::tool_error(&tool_use_id, result_text)
        };
        history.push(tool_msg);
    }

    (history, user_content)
}

// ── Prediction facade ───────────────────────────────────────────────────────

/// 预测失败原因，用于决定是否发送通知及日志级别。
#[derive(Debug)]
pub enum PredictionError {
    /// 30s 超时（首次冷启动可能较慢）。
    Timeout,
    /// Agent 执行返回错误。
    Failed(String),
}

/// Facade：基于现有对话历史预测用户下一步输入。
///
/// 此函数封装了 TUI 之前在 `acp_server/mod.rs` 内联的 Prediction 构造逻辑
/// （`BaseModelReactLLM::new` + `RetryableLLM::new` + `ReActAgent::new`），
/// 避免违反 CLAUDE.md [TRAP]：
///
/// > Agent 构建和执行统一通过 `nexum_acp::session::executor::execute_prompt()`。
/// > 禁止在 TUI 层直接构建 ReActAgent。
///
/// 构建一个 1 轮、无工具、无中间件的最小 agent，注入 `history`（应已过滤 System
/// 消息并限制条数），30 秒超时后返回文本或 [`PredictionError`]。
///
/// 调用方负责发送 `peri/prediction_ready` 通知（保留在 TUI 层以便复用 transport）。
pub async fn execute_prediction(
    provider: crate::provider::LlmProvider,
    history: Vec<BaseMessage>,
    cwd: &str,
) -> Result<String, PredictionError> {
    debug!(
        msg_count = history.len(),
        cwd, "Prediction facade: starting"
    );

    // 直接复用已构建的 LlmProvider（绕过 from_config）
    let base_llm = nexum_agent::llm::BaseModelReactLLM::new(provider.into_model());
    let llm =
        nexum_agent::llm::RetryableLLM::new(base_llm, nexum_agent::llm::RetryConfig::default());

    // 构建最小 agent（1 轮、无工具、无中间件）
    let directive = nexum_middlewares::subagent::build_prediction_directive();
    let agent = nexum_agent::agent::executor::ReActAgent::new(llm)
        .max_iterations(1)
        .with_system_prompt(directive);

    // 构造 state，注入对话历史
    let mut state = nexum_agent::agent::state::AgentState::new(cwd);
    for msg in &history {
        state.add_message(msg.clone());
    }

    debug!("Prediction facade: calling LLM");
    // 30 秒超时（首次冷启动可能较慢）
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        agent.execute(
            nexum_agent::agent::react::AgentInput::text("请根据以上对话预测用户下一步输入"),
            &mut state,
            None,
        ),
    )
    .await;

    match result {
        Ok(Ok(_output)) => {
            let text = extract_prediction_text(state.messages());
            if text.is_empty() {
                debug!("Prediction facade: LLM returned empty text");
            } else {
                debug!(%text, "Prediction facade: ready");
            }
            Ok(text)
        }
        Ok(Err(e)) => {
            debug!(error = %e, "Prediction facade: agent failed");
            Err(PredictionError::Failed(e.to_string()))
        }
        Err(_) => {
            debug!("Prediction facade: timed out (30s)");
            Err(PredictionError::Timeout)
        }
    }
}

/// 从 agent 执行后的 state 中提取最后一条非空 AI 消息文本。
///
/// 纯函数（不持有 lock、不 await），便于单元测试。文本两侧空白会被裁剪。
pub fn extract_prediction_text(messages: &[BaseMessage]) -> String {
    messages
        .iter()
        .rev()
        .find_map(|m| {
            if matches!(m, BaseMessage::Ai { .. }) {
                let t = m.content();
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            } else {
                None
            }
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "executor_test.rs"]
mod tests;

#[cfg(test)]
#[path = "executor_prediction_test.rs"]
mod prediction_tests;
