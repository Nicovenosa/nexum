//! -p/--print 非交互模式：单轮问答后自动退出

use std::sync::Arc;

use anyhow::Result;

use crate::cli_args::OutputFormat;

/// -p 模式执行入口
#[allow(clippy::too_many_arguments)]
pub async fn run_print(
    prompt: Option<String>,
    output_format: Option<String>,
    max_turns: Option<u32>,
    bare: bool,
    model_override: Option<String>,
    effort_override: Option<String>,
    permission_mode_str: Option<String>,
    skip_permissions: bool,
    allowed_tools: Vec<String>,
    disallowed_tools: Vec<String>,
    settings_path: Option<String>,
    cwd: Option<String>,
) -> Result<()> {
    let fmt: OutputFormat = match output_format.as_deref() {
        Some(s) => s.parse().map_err(|e: String| anyhow::anyhow!(e))?,
        None => OutputFormat::Text,
    };

    let prompt_text = match prompt {
        Some(p) => p,
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf.trim().to_string()
        }
    };

    if prompt_text.is_empty() {
        anyhow::bail!("无输入 prompt。用法: nexum -p \"你的问题\" 或 echo \"问题\" | nexum -p");
    }

    let _telemetry = nexum_agent::telemetry::init_tracing("nexum-print");

    // 加载配置
    let nexum_config = match &settings_path {
        Some(path) => {
            let p = std::path::Path::new(path);
            if p.exists() {
                nexum_tui::config::load_from(p)?
            } else {
                let v: serde_json::Value = serde_json::from_str(path)
                    .map_err(|e| anyhow::anyhow!("--settings 不是有效文件路径或 JSON: {e}"))?;
                // Materialización efímera de `--settings <json>`, por PID.
                //
                // Tenía nombre FIJO, y eso es un bug de producción, no un
                // detalle de tests: dos `nexum --settings` simultáneos escriben
                // el mismo archivo y cada uno puede terminar cargando la config
                // del otro. Acá el PID SÍ corresponde —a diferencia de la base
                // de threads, que es dato persistente del usuario— porque esto
                // vive lo que dura la invocación y no lo lee nadie más.
                // ALLOW justificado: lleva el PID en el nombre (línea de abajo) y
                // se borra después de leerlo. Es el arreglo del bug de nombre fijo.
                #[allow(clippy::disallowed_methods)]
                let tmp = std::env::temp_dir()
                    .join(format!("nexum-settings-override-{}.json", std::process::id()));
                std::fs::write(&tmp, serde_json::to_string_pretty(&v)?)?;
                let cargado = nexum_tui::config::load_from(&tmp);
                // No se deja basura: ya se leyó.
                let _ = std::fs::remove_file(&tmp);
                cargado?
            }
        }
        None => nexum_tui::config::load().unwrap_or_default(),
    };

    // 构建 provider
    let provider = nexum_tui::app::agent::LlmProvider::from_config(&nexum_config)
        .or_else(nexum_tui::app::agent::LlmProvider::from_env)
        .ok_or_else(|| {
            anyhow::anyhow!("未配置 LLM provider。请设置 ANTHROPIC_API_KEY 或 OPENAI_API_KEY")
        })?;

    // --model 覆盖
    let provider = if let Some(ref model_str) = model_override {
        nexum_tui::app::agent::LlmProvider::from_config_for_alias(&nexum_config, model_str)
            .unwrap_or(provider)
    } else {
        provider
    };

    let _ = (effort_override, max_turns, allowed_tools, disallowed_tools);

    let cwd = cwd
        .as_deref()
        .map(|c| std::path::Path::new(c).canonicalize())
        .transpose()?
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
        .to_string_lossy()
        .to_string();

    tracing::info!(
        provider = %provider.display_name(),
        model = %provider.model_name(),
        cwd = %cwd,
        output = ?fmt,
        "print mode starting"
    );

    // SPEC-SECURITY-001 (GAP-P0): seguro por defecto también en -p. El modo
    // no-interactivo NO puede pedir aprobación, así que sin override explícito
    // las acciones sensibles se rechazan (PrintBroker fail-closed). Para
    // ejecutar sin supervisión el usuario pasa --dangerously-skip-permissions
    // o --permission-mode bypass|auto-mode. Valor inválido → fail-safe Default.
    let permission_mode = if skip_permissions {
        nexum_middlewares::prelude::PermissionMode::Bypass
    } else if let Some(ref mode_str) = permission_mode_str {
        match mode_str.as_str() {
            "bypass" => nexum_middlewares::prelude::PermissionMode::Bypass,
            "default" => nexum_middlewares::prelude::PermissionMode::Default,
            "accept-edit" => nexum_middlewares::prelude::PermissionMode::AcceptEdit,
            "auto-mode" => nexum_middlewares::prelude::PermissionMode::AutoMode,
            _ => nexum_middlewares::prelude::PermissionMode::Default,
        }
    } else if nexum_middlewares::prelude::is_yolo_mode() {
        nexum_middlewares::prelude::PermissionMode::Bypass // YOLO_MODE=true explícito
    } else {
        nexum_middlewares::prelude::PermissionMode::Default
    };
    let shared_permission = nexum_middlewares::prelude::SharedPermissionMode::new(permission_mode);

    let cron_control = nexum_middlewares::cron::CronControlClient::unavailable();

    // MCP pool（bare 时跳过）
    let mcp_pool = if bare {
        None
    } else {
        let claude_home = dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".claude");
        let pool = Arc::new(nexum_middlewares::mcp::McpClientPool::new_pending());
        let pool_clone = pool.clone();
        let cwd_clone = cwd.clone();
        let (init_tx, _init_rx) =
            tokio::sync::watch::channel(nexum_middlewares::mcp::McpInitStatus::Pending);
        tokio::spawn(async move {
            nexum_middlewares::mcp::McpClientPool::run_initialize(
                pool_clone,
                std::path::Path::new(&cwd_clone),
                &claude_home,
                init_tx,
                None,
                None,
            )
            .await;
        });
        Some(pool)
    };

    // 插件（bare 时跳过）
    let (plugin_skill_roots, plugin_agent_dirs, hook_groups, plugin_lsp_servers) = if bare {
        (vec![], vec![], vec![], vec![])
    } else {
        let claude_dir = dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".claude");
        let plugin_data = nexum_middlewares::plugin::load_enabled_plugins_aggregated(&claude_dir);
        let mut hg: Vec<Vec<nexum_middlewares::hooks::RegisteredHook>> = Vec::new();
        if !plugin_data.all_hooks.is_empty() {
            hg.push(plugin_data.all_hooks.clone());
        }
        let global_hooks = nexum_middlewares::hooks::loader::load_global_settings_hooks();
        if !global_hooks.is_empty() {
            hg.push(global_hooks);
        }
        let local_hooks = nexum_middlewares::hooks::loader::load_settings_local_hooks(&cwd);
        if !local_hooks.is_empty() {
            hg.push(local_hooks);
        }
        (
            plugin_data.all_skill_roots,
            plugin_data.all_agent_dirs,
            hg,
            plugin_data.all_lsp_servers,
        )
    };

    let tool_search_index = Arc::new(nexum_middlewares::tool_search::ToolSearchIndex::new());
    let shared_tools = Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new()));

    // broker（自动批准所有）
    let broker: Arc<dyn nexum_agent::interaction::UserInteractionBroker> = Arc::new(PrintBroker);

    // EventSink 实现（收集事件）
    let collector = Arc::new(std::sync::Mutex::new(PrintCollector::new(fmt)));
    let event_sink: Arc<dyn nexum_acp::session::event_sink::EventSink> = {
        let c = collector.clone();
        Arc::new(PrintEventSink { collector: c })
    };

    let cancel = nexum_agent::agent::AgentCancellationToken::new();
    let nexum_config_arc = Arc::new(nexum_config);

    // 创建一次性 AgentPool（print 模式无跨 prompt 复用）
    let pool = Arc::new(parking_lot::Mutex::new(
        nexum_acp::session::agent_pool::AgentPool::new(),
    ));

    // execute_prompt 是同步函数（返回 PromptResult，不是 async）
    let result = nexum_acp::session::executor::execute_prompt(
        nexum_acp::session::executor::PromptExecutionContext {
            provider,
            nexum_config: nexum_config_arc,
            cwd,
            session_id: String::new(), // print 模式不需要
            cancel,
            event_sink,
            broker,
            permission_mode: shared_permission,
            content: nexum_agent::messages::MessageContent::text(prompt_text),
            stable_profile: false,
            task_envelope: None,
            frozen: None, // no frozen data
            history: vec![],
            incoming_recalls: vec![],
            session_start_source: Some("startup".to_string()),
            bg_results: vec![], // print 模式无后台任务
            plugin_skill_roots,
            plugin_agent_dirs,
            hook_groups,
            cron_control,
            mcp_pool,
            channel_state: None,
            tool_search_index,
            shared_tools,
            lsp_servers: plugin_lsp_servers,
            langfuse_session: None, // print 模式暂不启用
            pool,
            thread_store: None,    // print 模式不需要持久化
            thread_id: None,       // parent_thread_id
            session_manager: None, // print 模式不需要 cancel 级联
        },
    )
    .await;
    let mut c = collector.lock().unwrap();
    c.fill_from_result_messages(&result.messages);
    c.output_final(result.ok);

    Ok(())
}

/// Broker de -p (no-interactivo). SPEC-SECURITY-001 (GAP-P0): NO auto-aprueba.
/// Rechaza toda solicitud de aprobación (fail-closed) — en modo no-interactivo
/// nadie puede consentir. El modo Bypass/skip-permissions salta el gate ANTES
/// de llegar al broker, así que ejecutar sin supervisión requiere ese override
/// explícito. Nunca hay false completion: el rechazo es un resultado real.
struct PrintBroker;

#[async_trait::async_trait]
impl nexum_agent::interaction::UserInteractionBroker for PrintBroker {
    async fn request(
        &self,
        context: nexum_agent::interaction::InteractionContext,
    ) -> nexum_agent::interaction::InteractionResponse {
        match context {
            nexum_agent::interaction::InteractionContext::Approval { items } => {
                nexum_agent::interaction::InteractionResponse::Decisions(
                    items
                        .into_iter()
                        .map(|_| nexum_agent::interaction::ApprovalDecision::Reject {
                            reason: "modo no-interactivo sin --dangerously-skip-permissions: \
                                     acción sensible rechazada (fail-closed)"
                                .to_string(),
                            source: None,
                        })
                        .collect(),
                )
            }
            nexum_agent::interaction::InteractionContext::Questions { requests } => {
                nexum_agent::interaction::InteractionResponse::Answers(
                    requests
                        .into_iter()
                        .map(|q| nexum_agent::interaction::QuestionAnswer {
                            id: q.id,
                            selected: vec![],
                            text: Some(String::new()),
                        })
                        .collect(),
                )
            }
        }
    }
}

/// EventSink 实现：收集事件并输出
struct PrintEventSink {
    collector: Arc<std::sync::Mutex<PrintCollector>>,
}

#[async_trait::async_trait]
impl nexum_acp::session::event_sink::EventSink for PrintEventSink {
    async fn push_event(
        &self,
        _session_id: &str,
        event: &nexum_agent::agent::events::AgentEvent,
        _context_window: u32,
    ) {
        let mut c = self.collector.lock().unwrap();
        let output = c.handle_event(event.clone());
        if let Some(line) = output {
            println!("{}", line);
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    }

    async fn push_done(&self, _session_id: &str) {}
}

/// 事件收集器
struct PrintCollector {
    fmt: OutputFormat,
    text_buffer: String,
}

impl PrintCollector {
    fn new(fmt: OutputFormat) -> Self {
        Self {
            fmt,
            text_buffer: String::new(),
        }
    }

    fn handle_event(&mut self, event: nexum_agent::agent::AgentEvent) -> Option<String> {
        use nexum_agent::agent::AgentEvent as E;

        match self.fmt {
            OutputFormat::StreamJson => match event {
                E::TextChunk { chunk, .. } => Some(
                    serde_json::to_string(&serde_json::json!({
                        "type": "text",
                        "content": chunk
                    }))
                    .unwrap(),
                ),
                E::ToolStart {
                    tool_call_id, name, ..
                } => Some(
                    serde_json::to_string(&serde_json::json!({
                        "type": "tool_use",
                        "id": tool_call_id,
                        "name": name,
                        "input": null
                    }))
                    .unwrap(),
                ),
                E::ToolEnd {
                    tool_call_id,
                    output,
                    ..
                } => Some(
                    serde_json::to_string(&serde_json::json!({
                        "type": "tool_result",
                        "id": tool_call_id,
                        "output": output
                    }))
                    .unwrap(),
                ),
                _ => None,
            },
            OutputFormat::Text | OutputFormat::Json => match event {
                E::TextChunk { chunk, .. } => {
                    self.text_buffer.push_str(&chunk);
                    None
                }
                E::LlmCallEnd { output, .. } => {
                    if self.text_buffer.trim().is_empty() && !output.trim().is_empty() {
                        self.text_buffer = output;
                    }
                    None
                }
                _ => None,
            },
        }
    }

    fn fill_from_result_messages(&mut self, messages: &[nexum_agent::messages::BaseMessage]) {
        if !self.text_buffer.trim().is_empty() {
            return;
        }

        let Some(text) = messages.iter().rev().find_map(|message| match message {
            nexum_agent::messages::BaseMessage::Ai { .. } => {
                let content = message.content();
                if content.trim().is_empty() {
                    None
                } else {
                    Some(content)
                }
            }
            _ => None,
        }) else {
            return;
        };

        self.text_buffer = text;
    }

    fn output_final(&self, _ok: bool) {
        match self.fmt {
            OutputFormat::Text => {
                println!("{}", self.text_buffer);
            }
            OutputFormat::Json => {
                let result = serde_json::json!({
                    "type": "result",
                    "content": self.text_buffer,
                });
                println!("{}", serde_json::to_string_pretty(&result).unwrap());
            }
            OutputFormat::StreamJson => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexum_agent::messages::BaseMessage;

    #[test]
    fn test_fill_from_result_messages_uses_last_ai_when_empty() {
        let mut collector = PrintCollector::new(OutputFormat::Text);
        collector.fill_from_result_messages(&[
            BaseMessage::human("prompt"),
            BaseMessage::ai("Nexum online"),
        ]);
        assert_eq!(collector.text_buffer, "Nexum online");
    }

    #[test]
    fn test_fill_from_result_messages_preserves_streamed_text() {
        let mut collector = PrintCollector::new(OutputFormat::Text);
        collector.text_buffer = "streamed".to_string();
        collector.fill_from_result_messages(&[BaseMessage::ai("final")]);
        assert_eq!(collector.text_buffer, "streamed");
    }

    #[test]
    fn test_handle_llm_call_end_uses_output_when_no_text_chunk() {
        let mut collector = PrintCollector::new(OutputFormat::Text);
        collector.handle_event(nexum_agent::agent::AgentEvent::LlmCallEnd {
            step: 1,
            model: "test".to_string(),
            output: "Nexum online".to_string(),
            usage: None,
            stop_reason: None,
        });
        assert_eq!(collector.text_buffer, "Nexum online");
    }
}
