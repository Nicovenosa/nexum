//! MessagePipeline 生命周期管理。

use nexum_agent::messages::BaseMessage;

use crate::ui::message_view::MessageViewModel;

impl super::MessagePipeline {
    /// 标记当前 AI 轮次结束
    pub fn done(&mut self) {
        self.finalize_current_ai();
        self.current_ai_finalized = false;
        self.pending_tools.clear();
        self.completed_tools.clear();
        self.adaptive_policy.reset();
        self.force_flush_block();
        self.throttle_last_fire = None;
        self.active_batch = None;
        self.drain_subagent_stack();
    }

    /// 中断：finalize 当前状态并清理残留
    pub fn interrupt(&mut self) {
        self.finalize_current_ai();
        self.current_ai_finalized = false;
        self.pending_tools.clear();
        self.completed_tools.clear();
        self.adaptive_policy.reset();
        self.force_flush_block();
        self.throttle_last_fire = None;
        self.active_batch = None;
        self.drain_subagent_stack();
    }

    pub fn clear(&mut self) {
        self.completed.clear();
        self.current_ai_text.clear();
        self.current_ai_reasoning.clear();
        self.current_ai_tool_calls.clear();
        self.current_ai_finalized = false;
        self.pending_tools.clear();
        self.completed_tools.clear();
        self.subagent_stack.clear();
        self.frozen_subagent_vms.clear();
        self.active_batch = None;
    }

    /// 清空并释放所有内部 buffer 的 capacity
    pub fn shrink_to_fit(&mut self) {
        self.completed.shrink_to_fit();
        self.current_ai_text.shrink_to_fit();
        self.current_ai_reasoning.shrink_to_fit();
        self.current_ai_tool_calls.shrink_to_fit();
        self.pending_tools.shrink_to_fit();
        self.completed_tools.shrink_to_fit();
        self.subagent_stack.shrink_to_fit();
        self.frozen_subagent_vms.shrink_to_fit();
    }

    /// 当前 AI 消息是否有可见内容
    pub fn has_streaming_content(&self) -> bool {
        !self.current_ai_text.trim().is_empty() || !self.current_ai_reasoning.is_empty()
    }

    /// 当前 AI 消息是否有待处理的 tool_calls
    pub fn has_pending_tool_calls(&self) -> bool {
        !self.current_ai_tool_calls.is_empty()
    }

    /// 是否在 SubAgent 执行中
    pub fn in_subagent(&self) -> bool {
        // 后台 agent 不会阻塞父 agent 的 Done 事件
        self.subagent_stack
            .last()
            .is_some_and(|s| s.is_running && !s.is_background)
    }

    /// 本轮是否已收到过 StateSnapshot
    pub fn has_snapshot_this_round(&self) -> bool {
        self.has_snapshot_this_round
    }

    /// 诊断用：返回 frozen_subagent_vms 的数量
    pub fn frozen_subagent_vms_count(&self) -> usize {
        self.frozen_subagent_vms.len()
    }

    /// 可变访问 frozen_subagent_vms（供 handle_background_task_completed 同步更新状态）
    pub fn frozen_subagent_vms_mut(&mut self) -> &mut Vec<MessageViewModel> {
        &mut self.frozen_subagent_vms
    }

    // ── 轮次管理 ──────────────────────────────────────────────────────────────

    /// 标记新一轮对话开始。由 submit_message() 调用。
    pub fn begin_round(&mut self) {
        self.completed_len_at_round_start = self.completed.len();
        self.has_snapshot_this_round = false;
        self.adaptive_policy.reset();
        self.throttle_last_fire = None;
        // 清空上一轮的 frozen_subagent_vms，防止跨轮次累积导致新轮次的
        // SubAgentGroup 按位置错误匹配到旧轮的 frozen VM（而非本轮的）。
        self.frozen_subagent_vms.clear();
    }

    /// 获取已完成的 BaseMessages（用于持久化）
    pub fn completed_messages(&self) -> &[BaseMessage] {
        &self.completed
    }

    /// 从 pipeline 规范状态构建尾部 VMs。
    ///
    pub fn set_completed(&mut self, msgs: Vec<BaseMessage>) {
        // Transcript inmutable (fix chat 2026-07-06): si el usuario YA VIO
        // texto streameado y el snapshot entrante no lo trae en ningún
        // mensaje AI (providers OpenAI-compat que dejan `content` vacío
        // cuando el texto pre-herramientas viajó como reasoning_content),
        // se preserva como mensaje AI sintético ANTES de extender. Sin esto,
        // el clear() de abajo + RebuildAll borraban la respuesta visible
        // ("la respuesta inicial desapareció" — E2E real con MiMo/Zen).
        let streamed = self.current_ai_text.trim();
        if !streamed.is_empty() && !snapshot_contains_streamed_text(&msgs, streamed) {
            self.completed
                .push(BaseMessage::ai(self.current_ai_text.clone()));
        }
        self.completed.extend(msgs);
        self.current_ai_text.clear();
        self.current_ai_reasoning.clear();
        self.current_ai_tool_calls.clear();
        self.current_ai_finalized = true;
        self.has_snapshot_this_round = true;
        self.pending_tools.clear();
        self.completed_tools.clear();
    }

    /// 返回 completed 的条数和估算堆内存（字节），供 /gc 诊断用
    pub fn completed_stats(&self) -> (usize, usize) {
        let count = self.completed.len();
        let bytes = super::super::super::command::core::gc::estimate_messages_heap(&self.completed);
        (count, bytes)
    }

    /// 从外部加载全量 BaseMessages（用于历史恢复后覆盖），并清除所有状态
    pub fn restore_completed(&mut self, msgs: Vec<BaseMessage>) {
        self.completed = msgs;
        self.completed_len_at_round_start = self.completed.len();
        self.has_snapshot_this_round = false;
        self.current_ai_text.clear();
        self.current_ai_reasoning.clear();
        self.current_ai_tool_calls.clear();
        self.current_ai_finalized = true;
    }
}

/// Normaliza whitespace para comparación tolerante (streaming vs snapshot
/// pueden diferir en saltos de línea / espacios de markdown parcial).
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// True si algún mensaje AI del snapshot contiene el texto ya streameado
/// (comparación por prefijo normalizado de 80 chars — suficiente para
/// detectar "es el mismo texto" sin exigir igualdad byte a byte).
pub(crate) fn snapshot_contains_streamed_text(msgs: &[BaseMessage], streamed: &str) -> bool {
    let needle: String = normalize_ws(streamed).chars().take(80).collect();
    if needle.is_empty() {
        return true;
    }
    msgs.iter().any(|m| {
        matches!(m, BaseMessage::Ai { .. }) && normalize_ws(&m.content()).contains(&needle)
    })
}
