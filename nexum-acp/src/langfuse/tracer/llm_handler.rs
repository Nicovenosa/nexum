//! LLM Generation 事件处理：on_llm_start / on_llm_end / on_llm_retrying。
//!
//! 提取自原 tracer.rs（339-453、574-588 行）。委托 EventBuilder + Usage 子模块。

use langfuse_client::{GenerationBody, IngestionEvent};

use nexum_agent::{llm::types::TokenUsage, messages::BaseMessage, tools::ToolDefinition};

use super::context::RetryAttempt;
use super::event_builder::{new_uuid, now_rfc3339, try_add_or_warn, VERSION};
use super::usage::{build_retry_metadata, build_usage_details};
use super::LangfuseTracer;

impl LangfuseTracer {
    /// LLM 调用开始：提交上一轮工具批次 Span，缓存本轮 input
    pub fn on_llm_start(
        &mut self,
        step: usize,
        messages: &[BaseMessage],
        tools: &[ToolDefinition],
    ) {
        self.flush_tools_batch();
        let gen_id = new_uuid();
        let start_time = now_rfc3339();
        self.generation_data.insert(
            step,
            (gen_id, messages.to_vec(), tools.to_vec(), start_time),
        );
        self.active_step = Some(step);
        self.retry_attempts.clear();
    }

    /// LLM 调用结束：同步创建 Generation 事件
    pub fn on_llm_end(
        &mut self,
        step: usize,
        model: &str,
        provider: &str,
        output: &str,
        usage: Option<&TokenUsage>,
    ) {
        let Some((gen_id, messages, tools, start_time)) = self.generation_data.remove(&step) else {
            return;
        };
        let end_time = now_rfc3339();

        let messages_val = serde_json::to_value(&messages).unwrap_or_else(|e| {
            tracing::warn!(error = %e, trace_id = %self.trace_id, "langfuse: messages 序列化失败");
            serde_json::json!({ "error": "serialization failed", "detail": e.to_string() })
        });
        let tools_val = serde_json::to_value(&tools).unwrap_or_else(|e| {
            tracing::warn!(error = %e, trace_id = %self.trace_id, "langfuse: tools 序列化失败");
            serde_json::json!({ "error": "serialization failed", "detail": e.to_string() })
        });
        let input_json = serde_json::json!({
            "messages": messages_val,
            "tools": tools_val,
        });

        let langfuse_usage_details = usage.map(build_usage_details);

        let gen_metadata = build_retry_metadata(&self.retry_attempts);
        self.active_step = None;
        self.retry_attempts.clear();

        let body = GenerationBody {
            id: Some(gen_id.clone()),
            trace_id: Some(self.trace_id.clone()),
            name: Some(format!("Chat{}", provider)),
            input: Some(input_json),
            output: Some(serde_json::json!(output)),
            model: Some(model.to_string()),
            usage_details: langfuse_usage_details,
            parent_observation_id: Some(self.current_agent_id()),
            start_time: Some(start_time),
            end_time: Some(end_time.clone()),
            session_id: Some(self.session_id.clone()),
            version: Some(VERSION.to_string()),
            ..Default::default()
        };
        let event = IngestionEvent::GenerationCreate {
            id: gen_id.clone(),
            timestamp: end_time,
            body,
            metadata: gen_metadata,
        };
        try_add_or_warn(
            &self.session.batcher,
            event,
            &self.trace_id,
            &format!("generation (gen_id={})", gen_id),
        );
    }

    /// LLM 重试：记录重试信息，最终在 on_llm_end 时写入 Generation metadata
    pub fn on_llm_retrying(
        &mut self,
        attempt: usize,
        max_attempts: usize,
        delay_ms: u64,
        error: &str,
    ) {
        self.retry_attempts.push(RetryAttempt {
            attempt,
            max_attempts,
            delay_ms,
            error: error.to_string(),
        });
    }
}
