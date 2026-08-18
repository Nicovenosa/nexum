//! 工具调用事件处理：on_tool_start / on_tool_end / on_text_chunk。
//!
//! 提取自原 tracer.rs（149-151、455-572 行）。集中 on_tool_end 的借用 workaround。

use langfuse_client::{IngestionEvent, ObservationBody, ObservationType};

use super::context::PendingTool;
use super::event_builder::{new_uuid, now_rfc3339, try_add_or_warn, VERSION};
use super::LangfuseTracer;

impl LangfuseTracer {
    /// TextChunk 事件：累积最终回答
    pub fn on_text_chunk(&mut self, chunk: &str) {
        self.final_answer.push_str(chunk);
    }

    /// 工具调用开始
    pub fn on_tool_start(&mut self, tool_call_id: &str, name: &str, input: &serde_json::Value) {
        let tool_span_id;

        // [TRAP] Block 限定 current_tools_context 的可变借用范围。
        // 若直接让 EventBuilder 持有 &mut self 会加剧借用冲突，此处保持原样的
        // block scope workaround，EventBuilder 方法仅接收 owned 数据。
        {
            let current_agent_id = self.current_agent_id();
            let (batch_id_ref, start_time_ref, _, pending_tools) = self.current_tools_context();
            if pending_tools.is_empty() {
                *batch_id_ref = Some(new_uuid());
                *start_time_ref = Some(now_rfc3339());
            }
            let parent_span_id = batch_id_ref.clone().unwrap_or(current_agent_id);

            tool_span_id = new_uuid();
            let start_time = now_rfc3339();
            pending_tools.insert(
                tool_call_id.to_string(),
                PendingTool {
                    span_id: tool_span_id.clone(),
                    name: name.to_string(),
                    input: input.clone(),
                    start_time,
                    parent_span_id,
                },
            );
        } // 可变借用在此释放

        // Agent 工具：创建 SubAgent Span，push 到栈
        // [TRAP] is_agent 判断统一委托给 is_agent_tool()，与 on_tool_end 共享
        // 同一方法，避免两处 `name == "Agent"` 逻辑漂移。
        if self.is_agent_tool(tool_call_id) {
            self.begin_subagent(input);
        }
    }

    /// 工具调用结束：同步创建 tool observation
    pub fn on_tool_end(&mut self, tool_call_id: &str, output: &str, is_error: bool) {
        let session_id = self.session_id.clone();
        let trace_id = self.trace_id.clone();
        let trace_id_for_log = self.trace_id.clone();

        // [TRAP] Agent 工具的 PendingTool 在 on_tool_start 时插入到**父级** context
        //（begin_subagent push 前），而 current_tools_context() 会返回子 agent 的 context。
        // 因此必须先 end_subagent（pop 栈回到父级），再查找 PendingTool。
        // 拆分 ToolHandler 时不能颠倒此顺序。
        // is_agent 判断统一委托给 is_agent_tool()，与 on_tool_start 共享同一方法。
        if self.is_agent_tool(tool_call_id) {
            self.end_subagent(output, is_error);
        }

        let (_, _, end_time_ref, pending_tools) = self.current_tools_context();
        let Some(tool) = pending_tools.remove(tool_call_id) else {
            return;
        };
        let end_time = now_rfc3339();

        let tool_name = tool.name.clone();
        let span_id = tool.span_id;
        let tool_name_for_body = tool.name.clone();
        let tool_input = tool.input;
        let tool_start_time = tool.start_time;
        let tool_parent_id = tool.parent_span_id;

        let status_msg = if is_error {
            Some("error".to_string())
        } else {
            None
        };

        let body = ObservationBody {
            id: Some(span_id),
            trace_id: Some(trace_id),
            r#type: ObservationType::Tool,
            name: Some(tool_name_for_body),
            input: Some(tool_input),
            output: Some(serde_json::json!(output)),
            start_time: Some(tool_start_time),
            end_time: Some(end_time.clone()),
            completion_start_time: None,
            parent_observation_id: Some(tool_parent_id),
            metadata: None,
            model: None,
            model_parameters: None,
            level: None,
            status_message: status_msg,
            version: Some(VERSION.to_string()),
            environment: None,
            session_id: Some(session_id),
        };
        let event = IngestionEvent::ObservationCreate {
            id: new_uuid(),
            timestamp: end_time.clone(),
            body,
            metadata: None,
        };
        // [TRAP] 释放 current_tools_context 的可变借用。
        // end_time_ref / pending_tools 是借用 workaround 的痕迹而非语义需要。
        let _ = end_time_ref;
        let _ = pending_tools;

        try_add_or_warn(
            &self.session.batcher,
            event,
            &trace_id_for_log,
            &format!("tool observation (tool={})", tool_name),
        );

        // 重新获取可变借用
        let (_, _, end_time_ref, _) = self.current_tools_context();
        *end_time_ref = Some(end_time);
    }
}
