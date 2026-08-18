//! 对话轮次生命周期：on_trace_start / on_trace_end。
//!
//! 提取自原 tracer.rs（297-336、717-756 行）。on_trace_end 中的 tokio::spawn 是
//! 整个 Tracer 唯一的 async 路径（最终 flush）。

use std::sync::Arc;

use langfuse_client::{IngestionEvent, ObservationBody, ObservationType};

use super::event_builder::{new_uuid, now_rfc3339, VERSION};
use super::LangfuseTracer;

impl LangfuseTracer {
    /// 对话轮次开始：创建 agent-run Observation（根 observation）
    pub fn on_trace_start(&mut self, input: &str) {
        let batcher = &self.session.batcher;
        let start_time = now_rfc3339();
        tracing::info!(
            trace_id = %self.trace_id,
            agent_obs_id = %self.agent_observation_id,
            "langfuse: on_trace_start called"
        );

        // 创建 agent-run 根 Observation（OTLP 通过 trace_id 隐式创建 Trace，无需 TraceCreate）
        let body = ObservationBody {
            id: Some(self.agent_observation_id.clone()),
            trace_id: Some(self.trace_id.clone()),
            r#type: ObservationType::Agent,
            name: Some("agent-run".to_string()),
            start_time: Some(start_time),
            end_time: None,
            completion_start_time: None,
            parent_observation_id: None,
            input: Some(serde_json::json!(input)),
            output: None,
            metadata: None,
            model: None,
            model_parameters: None,
            level: None,
            status_message: None,
            version: Some(VERSION.to_string()),
            environment: None,
            session_id: Some(self.session_id.clone()),
        };
        let event = IngestionEvent::ObservationCreate {
            id: new_uuid(),
            timestamp: now_rfc3339(),
            body,
            metadata: None,
        };
        if let Err(e) = batcher.try_add(event) {
            tracing::warn!(error = %e, trace_id = %self.trace_id, "langfuse: agent-run observation 入队失败（背压丢弃）");
        }
    }

    /// 对话轮次结束：更新 agent-run Observation 输出和结束时间，并强制 flush。
    ///
    /// [不变量] 这是 Tracer 唯一的 async 路径（最终 flush）。所有其他事件
    /// 均通过 batcher.try_add() 同步入队，保证顺序。tokio::spawn 使 flush 异步化，
    /// 不阻塞调用方（executor.rs）。
    pub fn on_trace_end(&mut self, error_output: Option<&str>) -> tokio::task::JoinHandle<()> {
        self.flush_tools_batch();

        let batcher = Arc::clone(&self.session.batcher);
        let trace_id = self.trace_id.clone();
        let agent_observation_id = self.agent_observation_id.clone();
        let output = if let Some(err) = error_output {
            err.to_string()
        } else {
            std::mem::take(&mut self.final_answer)
        };

        tokio::spawn(async move {
            let end_time = now_rfc3339();

            // 更新 agent-run Observation 的 output 和 end_time
            let obs_body = ObservationBody {
                id: Some(agent_observation_id.clone()),
                trace_id: Some(trace_id.clone()),
                r#type: ObservationType::Agent,
                name: Some("agent-run".to_string()),
                output: Some(serde_json::json!(output)),
                end_time: Some(end_time.clone()),
                version: Some(VERSION.to_string()),
                ..Default::default()
            };
            let obs_event = IngestionEvent::ObservationUpdate {
                id: new_uuid(),
                timestamp: end_time,
                body: obs_body,
                metadata: None,
            };
            if let Err(e) = batcher.add(obs_event).await {
                tracing::warn!(error = %e, trace_id = %trace_id, obs_id = %agent_observation_id, "langfuse: agent-run observation 更新失败");
            }
            if let Err(e) = batcher.flush().await {
                tracing::warn!(error = %e, trace_id = %trace_id, "langfuse: batcher flush 失败");
            }
        })
    }
}
