use super::{message_pipeline::PipelineAction, *};

impl App {
    pub fn submit_message(&mut self, input: String) {
        if input.trim().is_empty() {
            return;
        }

        // ── TUI 本地命令拦截：/streaming ──
        if let Some(args) = input.strip_prefix("/streaming") {
            self.handle_streaming_command(args.trim());
            return;
        }

        if input.trim() == "/trace" {
            let trace = self
                .session_mgr
                .current()
                .metadata
                .last_turn_provenance
                .as_ref()
                .map(crate::app::turn_provenance::TurnProvenance::render)
                .unwrap_or_else(|| "Turn provenance\nNo completed turn is available.".to_string());
            self.apply_pipeline_action(PipelineAction::AddMessage(MessageViewModel::system(trace)));
            return;
        }

        match crate::app::turn_provenance::classify_execution_intent(&input) {
            crate::app::turn_provenance::ExecutionIntent::Identity => {
                self.push_input_history(input.clone());
                let identity = {
                    let cfg = self.services.nexum_config.read();
                    crate::app::runtime_identity::runtime_identity(&cfg)
                };
                let user_vm = MessageViewModel::user(input.clone());
                self.apply_pipeline_action(PipelineAction::AddMessage(user_vm));
                self.session_mgr.current_mut().metadata.last_human_message = Some(input);
                let mut reply = MessageViewModel::assistant();
                reply.append_chunk(&format!(
                    "{}\n\nExecution path: LOCAL_DIRECT\nLLM invoked: false",
                    crate::app::runtime_identity::identity_response(&identity)
                ));
                if let MessageViewModel::AssistantBubble { is_streaming, .. } = &mut reply {
                    *is_streaming = false;
                }
                self.apply_pipeline_action(PipelineAction::AddMessage(reply));
                self.session_mgr.current_mut().metadata.last_turn_provenance = Some(
                    crate::app::turn_provenance::TurnProvenance::local_direct(&identity),
                );
                self.session_mgr.current_mut().agent.turn_terminal =
                    Some(nexum_acp::session::terminal::TerminalState::Completed);
                self.session_mgr
                    .current_mut()
                    .agent
                    .prompt_restoration_count = 1;
                return;
            }
            crate::app::turn_provenance::ExecutionIntent::WebSearchUnavailable => {
                self.push_input_history(input.clone());
                let identity = {
                    let cfg = self.services.nexum_config.read();
                    crate::app::runtime_identity::runtime_identity(&cfg)
                };
                self.apply_pipeline_action(PipelineAction::AddMessage(MessageViewModel::user(
                    input.clone(),
                )));
                self.session_mgr.current_mut().metadata.last_human_message = Some(input);
                let mut reply = MessageViewModel::assistant();
                reply.append_chunk(
                    "Esta instalación no tiene una herramienta de búsqueda web activa para este turno.\n\n\
                     Execution path: REJECTED_BY_POLICY\n\
                     Web access: false\n\
                     Tools requested: [WebSearch]\n\
                     Tools executed: []",
                );
                if let MessageViewModel::AssistantBubble { is_streaming, .. } = &mut reply {
                    *is_streaming = false;
                }
                self.apply_pipeline_action(PipelineAction::AddMessage(reply));
                self.session_mgr.current_mut().metadata.last_turn_provenance = Some(
                    crate::app::turn_provenance::TurnProvenance::rejected_web(&identity),
                );
                self.session_mgr.current_mut().agent.turn_terminal =
                    Some(nexum_acp::session::terminal::TerminalState::RejectedByPolicy);
                self.session_mgr
                    .current_mut()
                    .agent
                    .prompt_restoration_count = 1;
                return;
            }
            crate::app::turn_provenance::ExecutionIntent::Llm => {}
        }

        // ── Interceptor de memoria explícita (SPEC-MEMORY-001, FASE 7) ──
        // Detección determinística (0 tokens) de "recordá que X": construye
        // una propuesta VISIBLE que exige /memoria confirmar. Jamás persiste
        // solo; jamás manda la frase al proveedor (el usuario está hablándole
        // a la memoria, no al modelo). Flag OFF ⇒ este bloque no corre y el
        // texto sigue su flujo normal (cero overhead, gate 8).
        if crate::memory_gateway::enabled(&self.global_ui.memory_gw)
            && !input.starts_with('/')
            && self
                .session_mgr
                .current()
                .metadata
                .pending_attachments
                .is_empty()
        {
            if let Some(contenido) = crate::memory_gateway::intent::parse_save_intent(&input) {
                self.push_input_history(input.clone());
                let user_vm = MessageViewModel::user(input.clone());
                self.apply_pipeline_action(PipelineAction::AddMessage(user_vm));
                self.session_mgr.current_mut().metadata.last_human_message = Some(input);
                crate::command::session::memoria::proponer_guardado(
                    self,
                    &contenido,
                    "user",
                    crate::memory_gateway::USER_SCOPE_ID,
                );
                self.render_rebuild();
                return;
            }
        }

        // ── Interceptor Hormiguero (Sprint 0, pasillo local) ──
        // Pre-route de triviales ANTES de gastar proveedor pago. Flag OFF
        // por defecto; cualquier problema (sidecar caído, timeout, breaker
        // abierto) es Passthrough y el flujo sigue EXACTAMENTE como hoy.
        // Nunca intercepta comandos slash ni mensajes con adjuntos.
        let stable_task_envelope = match crate::hormiguero::bridge().route_stable_text(&input, "es")
        {
            crate::hormiguero::RouteOutcome::LocalAnswer(answer) => {
                // Mismo patrón que el interceptor de identidad de arriba:
                // user bubble + assistant bubble local, sin tocar el agente.
                self.push_input_history(input.clone());
                let user_vm = MessageViewModel::user(input.clone());
                self.apply_pipeline_action(PipelineAction::AddMessage(user_vm));
                self.session_mgr.current_mut().metadata.last_human_message = Some(input);
                let mut reply = MessageViewModel::assistant();
                reply.append_chunk(&answer);
                if let MessageViewModel::AssistantBubble { is_streaming, .. } = &mut reply {
                    *is_streaming = false;
                }
                self.apply_pipeline_action(PipelineAction::AddMessage(reply));
                self.session_mgr.current_mut().agent.turn_terminal =
                    Some(nexum_acp::session::terminal::TerminalState::Completed);
                self.session_mgr
                    .current_mut()
                    .agent
                    .prompt_restoration_count = 1;
                self.set_loading(false);
                return;
            }
            crate::hormiguero::RouteOutcome::NeedsPaidAi(Some(envelope)) => Some(*envelope),
            crate::hormiguero::RouteOutcome::NeedsPaidAi(None)
            | crate::hormiguero::RouteOutcome::Passthrough => {
                self.fail_closed_stable(
                    input,
                    "No se pudo construir el contrato estable del turno. No se envió el prompt.",
                );
                return;
            }
        };

        // ── Interceptor de Planning (OMEGA Live Wiring, Fase 3) ──
        // Para tareas ESCALADAS (no triviales) y planificables, con flag
        // NEXUM_PLANNING on: pedir plan al sidecar → el Validator determinístico
        // lo gobierna → Rust CONSUME el plan validado como scaffold de ejecución.
        // Plan obligatorio inválido/ausente ⇒ FAIL-CLOSED (nunca ejecución
        // directa silenciosa). Rust conserva autoridad sobre tools/permisos/
        // providers; HITL sigue vigente. Flag OFF por defecto ⇒ cero overhead.
        let mut planning_scaffold: Option<String> = None;
        if stable_task_envelope.is_none()
            && crate::planning::planning_enabled()
            && !input.starts_with('/')
            && self
                .session_mgr
                .current()
                .metadata
                .pending_attachments
                .is_empty()
            && crate::planning::is_planning_eligible(&input)
        {
            let task_id = format!(
                "task-{}",
                &crate::planning::evidence::hash_text(&input)[..16]
            );
            let trace_id = format!(
                "trace-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let task_class = crate::planning::task_class_for(&input);
            // task_started (evidencia del ciclo de vida real del producto).
            crate::planning::evidence::record(&crate::planning::evidence::EvidenceEvent {
                trace_id: &trace_id,
                task_id: &task_id,
                plan_id: None,
                lifecycle: "task_started",
                component: "tui",
                provenance: "agent_submit",
                input_hash: &crate::planning::evidence::hash_text(&input),
                output_hash: "",
            });
            // Plan obligatorio para tareas planificables (required=true).
            match crate::planning::gateway()
                .request_plan(&input, task_class, "low", true, &trace_id, &task_id)
            {
                crate::planning::PlanDecision::Validated(env) => {
                    crate::planning::gateway().mark_consumed(
                        &trace_id,
                        &task_id,
                        &env.plan_id,
                        &env.provenance,
                    );
                    // Fase A: Cartero → Workers → Evidence. Rust selecciona cada
                    // paso, Cartero arma contexto mínimo tipado, el Worker recibe
                    // capability acotada bajo autorización de Rust (solo-lectura;
                    // escritura/exec/red se difieren a HITL), resultado tipado a
                    // Evidence. No usurpa la ejecución de tools del agente.
                    let _summary = crate::planning::orchestrate_plan_steps(&env, &input, &trace_id);
                    planning_scaffold = Some(env.execution_scaffold());
                }
                crate::planning::PlanDecision::Rejected { reason_code, .. } => {
                    // FAIL-CLOSED: el plan obligatorio no validó ⇒ no se ejecuta.
                    self.fail_closed_planning(
                        input,
                        &format!(
                            "El plan para esta tarea no pasó la validación determinística \
                             (código: {reason_code}). No se ejecutó ninguna acción. \
                             Reformulá el pedido con más detalle o dividilo en pasos."
                        ),
                    );
                    return;
                }
                crate::planning::PlanDecision::NeedsUserInput { reason_code } => {
                    self.fail_closed_planning(
                        input,
                        &format!(
                            "El planificador necesita más información para armar un plan \
                             seguro (código: {reason_code}). Agregá detalle al pedido."
                        ),
                    );
                    return;
                }
                crate::planning::PlanDecision::GatewayUnavailable { detail: _ } => {
                    // Plan obligatorio + sidecar no disponible ⇒ fail-closed (la
                    // misión prohíbe fallback silencioso a ejecución directa).
                    self.fail_closed_planning(
                        input,
                        "La planificación es obligatoria (NEXUM_PLANNING on) pero el \
                         planificador no está disponible. No se ejecutó ninguna acción. \
                         Verificá el sidecar del Hormiguero o desactivá NEXUM_PLANNING.",
                    );
                    return;
                }
            }
        }

        // 记录提交前的状态长度，用于中断时回滚 origin_messages
        self.session_mgr.current_mut().metadata.pre_submit_state_len =
            self.session_mgr.current_mut().agent.origin_messages.len();

        self.push_input_history(input.clone());

        // 消费待发送附件
        let attachments =
            std::mem::take(&mut self.session_mgr.current_mut().metadata.pending_attachments);

        // 构建用于显示的文字（附件摘要追加在末尾）
        let display = if attachments.is_empty() {
            input.clone()
        } else {
            self.services.lc.tr_args(
                "app-submit-attachments",
                &[
                    ("input".into(), input.clone().into()),
                    ("count".into(), (attachments.len() as i64).into()),
                ],
            )
        };

        // Texto que ve el AGENTE: si hubo plan validado, va con el scaffold del
        // plan como prefijo (el plan afecta la ejecución). El `display` que ve
        // el usuario NO cambia (su pedido original).
        let agent_text = match &planning_scaffold {
            Some(scaffold) => format!("{scaffold}\n\nPedido del usuario:\n{input}"),
            None => input.clone(),
        };

        // 构建发送给 LLM 的 MessageContent（含附件图片 blocks）
        let message_content = if attachments.is_empty() {
            nexum_agent::messages::MessageContent::text(agent_text.clone())
        } else {
            let mut blocks = vec![nexum_agent::messages::ContentBlock::text(
                agent_text.clone(),
            )];
            for att in attachments {
                blocks.push(nexum_agent::messages::ContentBlock::image_base64(
                    &att.media_type,
                    &att.base64_data,
                ));
            }
            nexum_agent::messages::MessageContent::Blocks(blocks)
        };
        self.session_mgr
            .current_mut()
            .messages
            .pipeline
            .begin_round();
        let user_vm = MessageViewModel::user(display.clone());
        self.apply_pipeline_action(PipelineAction::AddMessage(user_vm));
        // round_start_vm_idx 在 UserBubble 推入之后设置，
        // 确保 RebuildAll 不会截掉当前轮次的用户消息
        self.session_mgr.current_mut().messages.round_start_vm_idx =
            self.session_mgr.current_mut().messages.view_messages.len();
        self.session_mgr.current_mut().metadata.last_human_message = Some(display);
        self.session_mgr.current_mut().messages.last_submitted_text = Some(input.clone());
        self.session_mgr.current_mut().agent.turn_terminal = None;
        self.session_mgr
            .current_mut()
            .agent
            .prompt_restoration_count = 0;
        self.set_loading(true);
        self.session_mgr.current_mut().ui.scroll_offset = u16::MAX;
        self.session_mgr.current_mut().ui.scroll_follow = true;
        self.session_mgr.current_mut().todo_items.clear();

        // 开始计时新任务
        self.session_mgr.current_mut().agent.task_start_time = Some(std::time::Instant::now());
        self.session_mgr.current_mut().agent.last_task_duration = None;
        if self
            .session_mgr
            .current_mut()
            .agent
            .session_start_time
            .is_none()
        {
            self.session_mgr.current_mut().agent.session_start_time =
                Some(std::time::Instant::now());
        }

        let identity = {
            let cfg = self.services.nexum_config.read();
            crate::app::runtime_identity::runtime_identity(&cfg)
        };
        self.session_mgr.current_mut().metadata.last_turn_provenance = Some(
            crate::app::turn_provenance::TurnProvenance::llm_selection(&identity),
        );
        let provider = {
            let cfg_guard = self.services.nexum_config.read();
            agent::LlmProvider::from_config(&cfg_guard)
        };
        let provider = match provider.or_else(agent::LlmProvider::from_env) {
            Some(p) => p,
            None => {
                let message = self.services.lc.tr("app-no-provider-submit");
                let _ = self.handle_turn_terminal(
                    nexum_acp::session::terminal::TerminalState::Failed,
                    Some(&message),
                );
                return;
            }
        };
        let selected_provider = provider.display_name().to_string();
        let selected_model = provider.model_name().to_string();
        if identity.provider_id.is_empty() || identity.model_id != selected_model {
            let message = format!(
                "PROVIDER_MODEL_SELECTION_INVALID: provider_id={} selected_model={} runtime_model={}",
                identity.provider_id, identity.model_id, selected_model
            );
            let _ = self.handle_turn_terminal(
                nexum_acp::session::terminal::TerminalState::Failed,
                Some(&message),
            );
            return;
        }
        let execution_route = match crate::app::turn_provenance::resolve_execution_route(&identity)
        {
            Ok(route) => route,
            Err(error) => {
                let message = error.to_string();
                let _ = self.handle_turn_terminal(
                    nexum_acp::session::terminal::TerminalState::Failed,
                    Some(&message),
                );
                return;
            }
        };
        if execution_route.provider_id != identity.provider_id
            || execution_route.catalog_model_id != identity.model_id
        {
            let message = format!(
                "PROVIDER_MODEL_ROUTE_MISMATCH: selected={}/{} route={}/{}",
                identity.provider_id,
                identity.model_id,
                execution_route.provider_id,
                execution_route.catalog_model_id
            );
            let _ = self.handle_turn_terminal(
                nexum_acp::session::terminal::TerminalState::Failed,
                Some(&message),
            );
            return;
        }
        if let Some(provenance) = self
            .session_mgr
            .current_mut()
            .metadata
            .last_turn_provenance
            .as_mut()
        {
            provenance.record_route(&execution_route);
        }

        // 从 Provider 模型获取正确的 context_window（解决第三方 Provider 默认 200k 不准确问题）
        // 若启用 1M 上下文模式，则覆盖为 1,000,000
        {
            let mut model_cw = provider.context_window();
            if self
                .services
                .nexum_config
                .read()
                .config
                .context_1m
                .unwrap_or(false)
            {
                model_cw = 1_000_000;
            }
            if model_cw > 0 && self.session_mgr.current_mut().agent.context_window != model_cw {
                tracing::debug!(
                    old = self.session_mgr.current_mut().agent.context_window,
                    new = model_cw,
                    "context_window updated from provider model"
                );
                self.session_mgr.current_mut().agent.context_window = model_cw;
            }
        }

        // 防御性重置：上次 agent 任务若 SubAgentEnd 因通道溢出被丢弃，
        // subagent_depth 会永久 > 0，导致所有后续 TokenUsageUpdate 被过滤（ctx 显示为 0）
        self.session_mgr.current_mut().agent.subagent_depth = 0;
        self.session_mgr.current_mut().agent.agent_replied = false;
        self.session_mgr.current_mut().agent.reconcile_already_done = false;
        // 清理后台任务 continuation 状态（用户主动发消息时覆盖自动 continuation）
        self.session_mgr
            .current_mut()
            .agent
            .bg_task_state
            .reset_for_new_round();
        // 重置 LSP 诊断计数
        self.session_mgr.current_mut().agent.lsp_diagnostics.reset();

        // ── ACP-based agent submission (replaces direct run_universal_agent spawn) ──
        let cwd = self.services.cwd.clone();
        if let Some(ref acp_client) = self.acp_client {
            // Clone what we need for the async task
            let acp_client_clone = acp_client.clone();
            let model_clone = self.services.model_name.clone();
            let message_content_clone = message_content.clone();
            let cwd_clone = cwd.clone();
            let stable_task_envelope_clone = stable_task_envelope.clone();
            let selected_provider_clone = selected_provider.clone();
            let selected_model_clone = selected_model.clone();
            // 恢复的历史 thread_id：存在时用 load_session 加载历史上下文
            let existing_thread_id = self.session_mgr.current_mut().current_thread_id.clone();

            // Spawn the ACP calls as a background task — NEVER block the TUI event loop.
            // Events will arrive via acp_notification_rx and be processed by poll_agent().
            tokio::spawn(async move {
                let client = acp_client_clone;
                let active_session_id = if !client.has_session() {
                    if let Some(ref tid) = existing_thread_id {
                        tracing::info!(thread_id = %tid, "ACP submit: loading existing session...");
                        match client
                            .load_session(tid, &cwd_clone, Some(&model_clone))
                            .await
                        {
                            Ok(sid) => {
                                tracing::info!(session_id = %sid, "ACP submit: load_session succeeded");
                                sid
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "ACP submit: load_session FAILED");
                                client.report_turn_failure(format!("ACP load_session failed: {e}"));
                                return;
                            }
                        }
                    } else {
                        tracing::info!("ACP submit: no session, calling new_session...");
                        match client.new_session(&cwd_clone, Some(&model_clone)).await {
                            Ok(sid) => {
                                tracing::info!(session_id = %sid, "ACP submit: new_session succeeded");
                                sid
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "ACP submit: new_session FAILED");
                                client.report_turn_failure(format!("ACP new_session failed: {e}"));
                                return;
                            }
                        }
                    }
                } else if let Some(sid) = client.current_session_id() {
                    sid
                } else {
                    client.report_turn_failure("ACP active session identity was lost");
                    return;
                };
                let Some(source_envelope) = stable_task_envelope_clone else {
                    client.report_turn_failure(
                        "stable TUI route refused to send a prompt without taskEnvelope",
                    );
                    return;
                };
                let conversion_context =
                    crate::hormiguero::acp_adapter::EnvelopeConversionContext {
                        session_id: active_session_id.clone(),
                        thread_id: existing_thread_id
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_else(|| active_session_id.clone()),
                        workspace: cwd_clone.clone(),
                        selected_provider: selected_provider_clone,
                        selected_model: selected_model_clone,
                    };
                let task_envelope = match crate::hormiguero::acp_adapter::to_acp_task_envelope(
                    &source_envelope,
                    &conversion_context,
                ) {
                    Ok(envelope) => envelope,
                    Err(error) => {
                        tracing::error!(%error, "ACP submit: task envelope conversion FAILED");
                        client.report_turn_failure(format!(
                            "No se pudo convertir el contrato estable: {error}"
                        ));
                        return;
                    }
                };
                tracing::info!("ACP submit: calling prompt...");
                match client
                    .prompt_with_task_envelope(&message_content_clone, &task_envelope)
                    .await
                {
                    Ok(()) => tracing::info!("ACP submit: prompt completed"),
                    Err(e) => {
                        tracing::error!(error = %e, "ACP submit: prompt FAILED");
                        client.report_turn_failure(format!("ACP prompt failed: {e}"));
                    }
                }
            });
        } else {
            // Fallback: ACP client not available, show error
            tracing::error!("ACP client not initialized, cannot submit agent");
            let _ = self.handle_turn_terminal(
                nexum_acp::session::terminal::TerminalState::Failed,
                Some("El transporte ACP no está disponible. No se envió el prompt."),
            );
        }
    }

    /// 发送缓冲的 cron 消息（每次只发一条，其余留待后续 Done 周期发送）
    /// 多条独立 cron 任务不应合并为一个 LLM 消息，避免语义混淆
    pub(crate) fn flush_pending_messages(&mut self) {
        if let Some(msg) = self
            .session_mgr
            .current_mut()
            .messages
            .pending_messages
            .first()
            .cloned()
        {
            self.session_mgr
                .current_mut()
                .messages
                .pending_messages
                .remove(0);
            self.submit_message(msg);
        }
    }

    /// 提交后台任务 continuation（使用合成 AgentResult tool_use + tool_result 消息）
    ///
    /// 与 `submit_message` 不同，此方法通过 `prompt_with_bg_results` 将结构化
    /// 后台任务结果发送给 ACP server，由 executor 注入合成消息。
    pub(crate) fn submit_bg_continuation(
        &mut self,
        results: Vec<crate::app::agent_comm::BgTaskResult>,
    ) {
        if results.is_empty() {
            return;
        }

        // 记录提交前的状态长度，用于中断时回滚
        self.session_mgr.current_mut().metadata.pre_submit_state_len =
            self.session_mgr.current_mut().agent.origin_messages.len();

        // 构建 display 文本（用于 UserBubble 显示）
        let count = results.len();
        let display = self.services.lc.tr_args(
            "app-bg-continuation",
            &[("count".into(), (count as i64).into())],
        );

        self.session_mgr
            .current_mut()
            .messages
            .pipeline
            .begin_round();
        let user_vm = MessageViewModel::user(display.clone());
        self.apply_pipeline_action(PipelineAction::AddMessage(user_vm));
        self.session_mgr.current_mut().messages.round_start_vm_idx =
            self.session_mgr.current_mut().messages.view_messages.len();
        self.session_mgr.current_mut().metadata.last_human_message = Some(display);
        self.session_mgr.current_mut().messages.last_submitted_text = None; // bg continuation 不恢复到输入框
        self.set_loading(true);
        self.session_mgr.current_mut().ui.scroll_offset = u16::MAX;
        self.session_mgr.current_mut().ui.scroll_follow = true;
        self.session_mgr.current_mut().todo_items.clear();

        // 开始计时新任务
        self.session_mgr.current_mut().agent.task_start_time = Some(std::time::Instant::now());
        self.session_mgr.current_mut().agent.last_task_duration = None;
        if self
            .session_mgr
            .current_mut()
            .agent
            .session_start_time
            .is_none()
        {
            self.session_mgr.current_mut().agent.session_start_time =
                Some(std::time::Instant::now());
        }

        // 重置状态
        self.session_mgr.current_mut().agent.subagent_depth = 0;
        self.session_mgr.current_mut().agent.agent_replied = false;
        self.session_mgr.current_mut().agent.reconcile_already_done = false;
        self.session_mgr.current_mut().agent.lsp_diagnostics.reset();

        // 通过 ACP client 提交 bg continuation
        if let Some(ref acp_client) = self.acp_client {
            let acp_client_clone = acp_client.clone();
            tokio::spawn(async move {
                match acp_client_clone.prompt_with_bg_results(results).await {
                    Ok(()) => {
                        tracing::info!("ACP bg continuation: prompt_with_bg_results completed")
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "ACP bg continuation: prompt_with_bg_results FAILED")
                    }
                }
            });
        } else {
            tracing::error!("ACP client not initialized, cannot submit bg continuation");
            self.apply_pipeline_action(PipelineAction::AddMessage(MessageViewModel::system(
                self.services.lc.tr("app-no-provider-submit"),
            )));
            self.set_loading(false);
        }
    }

    /// 处理 `/streaming` 本地命令：查看或切换流式渲染模式。
    fn handle_streaming_command(&mut self, args: &str) {
        use crate::app::message_pipeline::StreamingMode;

        let (mode, label) = match args {
            "" => {
                let current = self
                    .session_mgr
                    .current()
                    .messages
                    .pipeline
                    .streaming_mode();
                let mode_str = match current {
                    StreamingMode::Streaming => "Streaming",
                    StreamingMode::Block => "Block",
                    StreamingMode::None => "None",
                };
                let msg = format!(
                    "当前渲染模式：{}（可选：streaming / block / none）",
                    mode_str
                );
                self.apply_pipeline_action(PipelineAction::AddMessage(MessageViewModel::system(
                    msg,
                )));
                return;
            }
            "streaming" => (StreamingMode::Streaming, "Streaming"),
            "block" => (StreamingMode::Block, "Block"),
            "none" => (StreamingMode::None, "None"),
            _ => {
                self.apply_pipeline_action(PipelineAction::AddMessage(MessageViewModel::system(
                    "用法：/streaming [streaming|block|none]".to_string(),
                )));
                return;
            }
        };

        self.session_mgr
            .current_mut()
            .messages
            .pipeline
            .set_streaming_mode(mode);

        // 如果有 block buffer 残留需要 flush
        if self
            .session_mgr
            .current()
            .messages
            .pipeline
            .has_pending_block_flush()
        {
            let prefix = self.session_mgr.current().messages.round_start_vm_idx;
            if let Some(action) = self
                .session_mgr
                .current_mut()
                .messages
                .pipeline
                .check_throttle(prefix)
            {
                self.apply_pipeline_action(action);
            }
        }

        let msg = format!("渲染模式已切换为：{}", label);
        self.apply_pipeline_action(PipelineAction::AddMessage(MessageViewModel::system(msg)));
    }

    /// Fail-closed de planning (OMEGA Live Wiring): muestra el pedido del
    /// usuario + una nota que explica por qué NO se ejecutó, y corta el flujo
    /// sin ir al proveedor. La misión prohíbe el fallback silencioso a
    /// ejecución directa cuando el plan era obligatorio.
    fn fail_closed_planning(&mut self, input: String, reason: &str) {
        self.push_input_history(input.clone());
        let user_vm = MessageViewModel::user(input.clone());
        self.apply_pipeline_action(PipelineAction::AddMessage(user_vm));
        self.session_mgr.current_mut().metadata.last_human_message = Some(input);
        self.apply_pipeline_action(PipelineAction::AddMessage(MessageViewModel::system(
            reason.to_string(),
        )));
        self.render_rebuild();
    }

    /// Fallo cerrado temprano del perfil estable. Aunque todavía no exista
    /// una sesión ACP, converge al mismo terminal idempotente y devuelve el
    /// prompt al compositor una única vez.
    fn fail_closed_stable(&mut self, input: String, reason: &str) {
        self.push_input_history(input.clone());
        self.session_mgr
            .current_mut()
            .messages
            .pipeline
            .begin_round();
        self.apply_pipeline_action(PipelineAction::AddMessage(MessageViewModel::user(
            input.clone(),
        )));
        self.session_mgr.current_mut().metadata.last_human_message = Some(input.clone());
        self.session_mgr.current_mut().messages.last_submitted_text = Some(input);
        self.session_mgr.current_mut().agent.turn_terminal = None;
        self.session_mgr
            .current_mut()
            .agent
            .prompt_restoration_count = 0;
        self.set_loading(true);
        let _ = self.handle_turn_terminal(
            nexum_acp::session::terminal::TerminalState::Failed,
            Some(reason),
        );
    }
}
