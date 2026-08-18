//! Agent polling functions — poll_agent and poll_background_events.
//! Extracted from original agent_ops.rs (2026-05-20 split).

use crate::app::App;

impl App {
    pub fn poll_agent(&mut self) -> bool {
        // Cancel 超时安全网：5 秒后仍未收到 Interrupted/Done，强制清理
        if let Some(cancel_at) = self.session_mgr.current_mut().agent.cancel_sent_at {
            if cancel_at.elapsed() > std::time::Duration::from_secs(5)
                && self.session_mgr.current_mut().ui.loading
            {
                tracing::warn!(
                    "cancel timeout: 5s elapsed without Interrupted/Done, force cleanup"
                );
                self.session_mgr.current_mut().agent.cancel_sent_at = None;
                let _ = self.handle_turn_terminal(
                    nexum_acp::session::terminal::TerminalState::Cancelled,
                    Some("La cancelación no recibió confirmación del runtime."),
                );
                return true;
            }
        }
        // 优先处理延迟的后台任务 continuation（由 BackgroundTaskCompleted 处理器设置）
        // 只有在 loading=false 时才 take()，避免 loading=true（如 compact 中）时
        // continuation 被消费但未使用而永久丢失
        if !self.session_mgr.current_mut().ui.loading {
            if let Some(results) = self
                .session_mgr
                .current_mut()
                .agent
                .bg_task_state
                .pending_continuation
                .take()
            {
                tracing::info!("auto-submitting background task continuation with tool results");
                self.submit_bg_continuation(results);
                return true;
            }
        }

        // Check for events from ACP notification channel (primary path)
        let has_acp = self
            .session_mgr
            .current_mut()
            .agent
            .acp_notification_rx
            .is_some();

        if !has_acp {
            return false;
        }

        let mut updated = false;

        // 节流检查（每帧开始时，确保上一批 chunk 的尾部也被显示）
        {
            let prefix_len = self.session_mgr.current_mut().messages.round_start_vm_idx;
            if let Some(action) = self
                .session_mgr
                .current_mut()
                .messages
                .pipeline
                .check_throttle(prefix_len)
            {
                self.apply_pipeline_action(action);
                updated = true;
            }
        }

        loop {
            // Try ACP notification channel first (new path)
            let acp_result = self
                .session_mgr
                .current_mut()
                .agent
                .acp_notification_rx
                .as_mut()
                .map(|rx| rx.try_recv());
            if let Some(Ok(notif)) = acp_result {
                let (ev_updated, should_break, should_return) = self.handle_acp_notification(notif);
                if ev_updated {
                    updated = true;
                }
                if should_return {
                    return true;
                }
                if should_break {
                    break;
                }
                continue;
            }
            break;
        }

        // 当 loading=true 时（如 compact 中），即使没有新事件也返回 true，
        // 确保 spinner 动画持续渲染而非冻结
        let loading = self.session_mgr.current_mut().ui.loading;
        if loading {
            return true;
        }

        // Poll channel notifications
        self.poll_channel_notifications();

        updated
    }

    /// 每帧调用：消费 channel 消息通知，agent 空闲时直接提交，
    /// agent 运行中时缓冲到 pending_messages。
    fn poll_channel_notifications(&mut self) {
        const MAX_PENDING: usize = 10;

        // Drain notifications from channel receiver first (no self borrow across submit)
        let mut channel_notifications = Vec::new();
        {
            let session = &mut self.session_mgr.current_mut();
            if let Some(ref mut rx) = session.messages.channel_notification_rx {
                while let Ok(notif) = rx.try_recv() {
                    channel_notifications.push(notif);
                }
            }
        }

        for notif in channel_notifications {
            let xml = format!(
                r#"<channel source="{}" chat_id="{}">{}</channel>"#,
                notif.source, notif.chat_id, notif.text
            );

            let loading = self.session_mgr.current_mut().ui.loading;
            if !loading {
                // Agent is idle: submit immediately
                self.submit_message(xml);
            } else {
                let pending_messages =
                    &mut self.session_mgr.current_mut().messages.pending_messages;
                if pending_messages.len() < MAX_PENDING {
                    tracing::debug!(source = %notif.source, "channel 消息排队（agent 运行中）");
                    pending_messages.push(xml);
                } else {
                    tracing::warn!(
                        "pending_messages 已达上限 {}，丢弃 channel 消息",
                        MAX_PENDING
                    );
                }
            }
        }
    }

    /// 每帧调用：消费后台事件通道（MCP OAuth 等异步任务发送的事件），返回是否有 UI 更新
    pub fn poll_background_events(&mut self) -> bool {
        let events: Vec<_> = match self.services.bg_event_rx.as_mut() {
            Some(rx) => {
                let mut evts = Vec::new();
                loop {
                    match rx.try_recv() {
                        Ok(event) => evts.push(event),
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            self.services.bg_event_rx = None;
                            break;
                        }
                    }
                }
                evts
            }
            None => return false,
        };
        let mut updated = false;
        for event in events {
            let (ev_updated, _should_break, should_return) = self.handle_agent_event(event);
            if ev_updated {
                updated = true;
            }
            if should_return {
                return true;
            }
        }
        updated
    }

    /// 轮询 panic hook 通知通道，返回是否有新消息。
    /// panic 消息通过 tracing::error! 写入日志，同时通过通道通知 TUI 显示。
    pub fn poll_panic_notifications(&mut self) -> bool {
        // 先收集所有消息，释放 services 的借用
        let messages: Vec<String> = {
            let rx = match self.services.panic_notify_rx.as_mut() {
                Some(rx) => rx,
                None => return false,
            };
            let mut msgs = Vec::new();
            loop {
                match rx.try_recv() {
                    Ok(msg) => msgs.push(msg),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        self.services.panic_notify_rx = None;
                        break;
                    }
                }
            }
            msgs
        };
        let updated = !messages.is_empty();
        for msg in messages {
            self.push_system_note(msg);
            self.request_rebuild();
        }
        updated
    }

    /// 每帧调用：@ mention 已改为同步刷新，此处为 no-op（保留接口兼容）
    pub fn poll_at_mention(&mut self) -> bool {
        false
    }
}
