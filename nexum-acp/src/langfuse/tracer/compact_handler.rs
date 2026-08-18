//! Compact 操作 Span 追踪：on_compact_start / on_compact_end。
//!
//! 提取自原 tracer.rs（590-714 行）。区分 micro/full compact，构造不同 output JSON。

use langfuse_client::{IngestionEvent, SpanBody};

use super::context::CompactSpanContext;
use super::event_builder::{new_uuid, now_rfc3339, try_add_or_warn, VERSION};
use super::LangfuseTracer;

impl LangfuseTracer {
    /// Compact 开始：创建 compact Span（子 span 挂载到当前 agent observation）
    pub fn on_compact_start(&mut self) {
        let span_id = new_uuid();
        let start_time = now_rfc3339();

        let body = SpanBody {
            id: Some(span_id.clone()),
            trace_id: Some(self.trace_id.clone()),
            name: Some("compact".to_string()),
            start_time: Some(start_time.clone()),
            end_time: None,
            parent_observation_id: Some(self.current_agent_id()),
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
            timestamp: start_time.clone(),
            body,
            metadata: None,
        };
        try_add_or_warn(&self.session.batcher, event, &self.trace_id, "compact span");

        self.compact_span = Some(CompactSpanContext {
            span_id,
            start_time,
        });
    }

    /// Compact 完成/错误：更新 compact Span 的 output + end_time（或 error status）
    ///
    /// `summary`: full compact 时为摘要文本，micro compact 时为空
    /// `files_count`: 保留的文件数量
    /// `skills_count`: 保留的 Skill 数量
    /// `micro_cleared`: >0 表示 micro compact（清除的工具结果数）
    /// `is_error`: 是否为压缩失败
    /// `error_message`: 失败时的错误信息
    pub fn on_compact_end(
        &mut self,
        summary: &str,
        files_count: usize,
        skills_count: usize,
        micro_cleared: usize,
        is_error: bool,
        error_message: &str,
    ) {
        let Some(ctx) = self.compact_span.take() else {
            return;
        };
        let end_time = now_rfc3339();

        let compact_type = if micro_cleared > 0 { "micro" } else { "full" };
        // [CJK 截断] summary preview 必须用字符级操作（CLAUDE.md：&s[..N] 对 CJK 会 panic）
        let summary_preview: String = summary.chars().take(200).collect();

        let output = if is_error {
            serde_json::json!({
                "type": compact_type,
                "error": error_message,
            })
        } else if micro_cleared > 0 {
            serde_json::json!({
                "type": compact_type,
                "micro_cleared": micro_cleared,
            })
        } else {
            serde_json::json!({
                "type": compact_type,
                "summary": summary_preview,
                "files_count": files_count,
                "skills_count": skills_count,
            })
        };

        let status_message = if is_error {
            Some(if error_message.is_empty() {
                "error".to_string()
            } else {
                error_message.to_string()
            })
        } else {
            None
        };

        let body = SpanBody {
            id: Some(ctx.span_id),
            trace_id: Some(self.trace_id.clone()),
            name: Some("compact".to_string()),
            start_time: Some(ctx.start_time),
            end_time: Some(end_time.clone()),
            parent_observation_id: Some(self.current_agent_id()),
            input: None,
            output: Some(output),
            status_message,
            metadata: if !is_error && micro_cleared == 0 {
                Some(serde_json::json!({
                    "summary_full": summary,
                    "files_count": files_count,
                    "skills_count": skills_count,
                }))
            } else {
                None
            },
            level: None,
            version: Some(VERSION.to_string()),
            environment: None,
            session_id: Some(self.session_id.clone()),
        };
        let event = IngestionEvent::SpanUpdate {
            id: new_uuid(),
            timestamp: end_time,
            body,
            metadata: None,
        };
        try_add_or_warn(
            &self.session.batcher,
            event,
            &self.trace_id,
            "compact span 更新",
        );
    }
}
