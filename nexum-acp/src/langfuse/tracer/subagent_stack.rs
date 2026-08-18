//! SubAgent 嵌套栈管理。
//!
//! 封装 push/pop、current_agent_id、current_tools_context 等栈操作；集中 end_subagent
//! 的三步顺序约束与 begin_subagent 的上下文切换。原 tracer.rs 中 on_tool_end 的
//! is_agent 双 HashMap 查找逻辑保留于此（pop 后才返回父级 context）。
//!
//! 注意：本文件的方法均为 LangfuseTracer 的 impl 块（Rust 允许多个 impl 块）。

use std::collections::HashMap;

use langfuse_client::{IngestionEvent, ObservationBody, ObservationType};

use super::context::{PendingTool, SubAgentContext};
use super::event_builder::{new_uuid, now_rfc3339, try_add_or_warn, VERSION};
use super::LangfuseTracer;

impl LangfuseTracer {
    /// 查询 `tool_call_id` 是否对应 Agent 工具调用。
    ///
    /// 搜索两层：当前 pending_tools（父级 context）和 subagent_stack 中
    /// 每个子 agent 的 pending_tools。`on_tool_start` 和 `on_tool_end` 共享此方法，
    /// 避免两处 `name == "Agent"` 判断逻辑漂移。
    pub(crate) fn is_agent_tool(&self, tool_call_id: &str) -> bool {
        self.pending_tools
            .get(tool_call_id)
            .map(|t| t.name == "Agent")
            .unwrap_or(false)
            || self.subagent_stack.iter().any(|ctx| {
                ctx.pending_tools
                    .get(tool_call_id)
                    .map(|t| t.name == "Agent")
                    .unwrap_or(false)
            })
    }

    /// 获取当前活动的 agent observation ID
    /// 若有 subagent 栈，返回栈顶的 subagent ID；否则返回主 agent ID
    pub(crate) fn current_agent_id(&self) -> String {
        self.subagent_stack
            .last()
            .map(|ctx| ctx.observation_id.clone())
            .unwrap_or_else(|| self.agent_observation_id.clone())
    }

    /// 获取当前活动的 tools batch 上下文
    pub(crate) fn current_tools_context(
        &mut self,
    ) -> (
        &mut Option<String>,
        &mut Option<String>,
        &mut Option<String>,
        &mut HashMap<String, PendingTool>,
    ) {
        if let Some(subagent) = self.subagent_stack.last_mut() {
            (
                &mut subagent.tools_batch_span_id,
                &mut subagent.tools_batch_start_time,
                &mut subagent.tools_batch_end_time,
                &mut subagent.pending_tools,
            )
        } else {
            (
                &mut self.tools_batch_span_id,
                &mut self.tools_batch_start_time,
                &mut self.tools_batch_end_time,
                &mut self.pending_tools,
            )
        }
    }

    /// 从 Agent 工具的输入 JSON 中提取 subagent 标识（用于 Langfuse 显示名称）
    pub(crate) fn subagent_identity(input: &serde_json::Value) -> String {
        input
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                input
                    .get("fork")
                    .and_then(|v| v.as_bool())
                    .filter(|&f| f)
                    .map(|_| "fork".to_string())
            })
            .unwrap_or_else(|| "fork".to_string())
    }

    /// 创建 SubAgent 上下文并压入 subagent_stack
    ///
    /// Observation 延迟到 end_subagent 时发送，确保与 Tools batch 在同一批次，
    /// 避免周期性 flush 导致 parent 缺失引发 Langfuse 重复 trace。
    pub(crate) fn begin_subagent(&mut self, input: &serde_json::Value) {
        let agent_id = Self::subagent_identity(input);
        // [CJK 截断] prompt 预览必须用字符级操作（CLAUDE.md：&s[..N] 对 CJK 会 panic）
        let task_preview: String = input
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.chars().take(200).collect())
            .unwrap_or_default();

        let observation_id = new_uuid();
        let start_time = now_rfc3339();

        self.subagent_stack.push(SubAgentContext {
            observation_id,
            agent_id,
            start_time,
            input: serde_json::json!(task_preview),
            tools_batch_span_id: None,
            tools_batch_start_time: None,
            tools_batch_end_time: None,
            pending_tools: HashMap::new(),
        });
    }

    /// 完成当前 SubAgent Observation（Span 类型）：先发 ObservationCreate（确保 parent 先入队），
    /// 再 flush 工具批次，最后弹出栈。
    ///
    /// [TRAP] 必须在 `subagent_stack.pop()` 之前调用 `flush_tools_batch()`，
    /// 否则 subagent 的工具批次会 flush 到错误的 parent，导致 Langfuse 重复 trace。
    pub(crate) fn end_subagent(&mut self, result: &str, is_error: bool) {
        // 先发 SubAgent ObservationCreate，再 flush Tools batch
        // 确保 Tools SpanCreate 的 parent（subagent observation）先于它入队
        let status_message = if is_error {
            Some("error".to_string())
        } else {
            None
        };
        let end_time = now_rfc3339();

        if let Some(ctx) = self.subagent_stack.last() {
            let obs_body = ObservationBody {
                id: Some(ctx.observation_id.clone()),
                trace_id: Some(self.trace_id.clone()),
                r#type: ObservationType::Agent,
                name: Some(format!("subagent:{}", ctx.agent_id)),
                start_time: Some(ctx.start_time.clone()),
                end_time: Some(end_time.clone()),
                completion_start_time: None,
                parent_observation_id: None,
                input: Some(ctx.input.clone()),
                output: Some(serde_json::json!(result)),
                metadata: None,
                model: None,
                model_parameters: None,
                level: None,
                status_message,
                version: Some(VERSION.to_string()),
                environment: None,
                session_id: Some(self.session_id.clone()),
            };
            let obs_event = IngestionEvent::ObservationCreate {
                id: new_uuid(),
                timestamp: end_time,
                body: obs_body,
                metadata: None,
            };
            try_add_or_warn(
                &self.session.batcher,
                obs_event,
                &self.trace_id,
                &format!("subagent observation create (subagent={})", ctx.agent_id),
            );
        }

        // flush subagent 下的 tools batch（pop 前）
        self.flush_tools_batch();

        if self.subagent_stack.pop().is_none() {
            tracing::warn!("langfuse: end_subagent 调用时 subagent_stack 为空，忽略");
        }
    }

    /// 提交当前批次 Tools Span
    pub(crate) fn flush_tools_batch(&mut self) {
        let (batch_id, batch_start, batch_end, parent_id) = {
            let (batch_id_ref, batch_start_ref, batch_end_ref, _) = self.current_tools_context();
            if let (Some(batch_id), Some(batch_start), Some(batch_end)) = (
                batch_id_ref.take(),
                batch_start_ref.take(),
                batch_end_ref.take(),
            ) {
                (batch_id, batch_start, batch_end, self.current_agent_id())
            } else {
                return;
            }
        };

        let body = langfuse_client::SpanBody {
            id: Some(batch_id.clone()),
            trace_id: Some(self.trace_id.clone()),
            name: Some("Tools".to_string()),
            start_time: Some(batch_start),
            end_time: Some(batch_end.clone()),
            parent_observation_id: Some(parent_id),
            input: None,
            output: None,
            status_message: None,
            metadata: None,
            level: None,
            version: Some(VERSION.to_string()),
            environment: None,
            session_id: Some(self.session_id.clone()),
        };
        let event = IngestionEvent::SpanCreate {
            id: new_uuid(),
            timestamp: batch_end,
            body,
            metadata: None,
        };
        try_add_or_warn(
            &self.session.batcher,
            event,
            &self.trace_id,
            "tools batch span",
        );
    }
}
