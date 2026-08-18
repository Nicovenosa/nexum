//! SubAgent state tracking — token usage updates + subagent start events.
//! Extracted from original agent_ops.rs (2026-05-20 split).

use super::super::*;
use crate::app::{message_pipeline::PipelineAction, App};

/// Piso de eficiencia de cache bajo el cual se anota en la traza.
///
/// REGLA GENERAL: ningún umbral dispara con cero muestras. Sin denominador no
/// hay tasa, y comparar 0.0 contra un piso convierte "no sé" en "está mal".
/// Todo chequeo con esta forma se guarda con `total > 0` ANTES de comparar.
const CACHE_HIT_RATE_FLOOR: f64 = 0.8;

impl App {
    pub(super) fn handle_token_usage_update(
        &mut self,
        usage: nexum_agent::llm::types::TokenUsage,
    ) -> (bool, bool, bool) {
        // SubAgent 的 TokenUsageUpdate 不应污染父 agent 的 tracker
        if self.session_mgr.current_mut().agent.subagent_depth > 0 {
            return (true, false, false);
        }

        // 累积到会话追踪器
        self.session_mgr
            .current_mut()
            .agent
            .session_token_tracker
            .accumulate(&usage);

        // Eficiencia de cache: SÓLO a la traza. No va al stream de chat.
        //
        // Antes se dibujaba como aviso ⚠ en la conversación y estaba mal por
        // tres razones distintas:
        //
        //   1. Disparaba sin denominador. `cache_hit_rate()` devuelve 0.0
        //      cuando input_tokens == 0, o sea que CERO MUESTRAS da 0%, que es
        //      < 80%, así que alertaba. Un 0% sobre cero muestras no es un
        //      hallazgo: es ausencia de datos disfrazada de alerta.
        //   2. Es métrica de COSTO, no de correctitud. Nada que el usuario
        //      pueda decidir o accionar, y menos en un modelo gratuito.
        //   3. Competía visualmente con la respuesta, que es lo que el usuario
        //      está esperando.
        //
        // El criterio para el stream de chat: sólo lo que BLOQUEA al usuario o
        // requiere una decisión suya. "Nunca en silencio" es sobre lo que le
        // impide trabajar, no sobre telemetría. Esto se consulta en /trace.
        // `None` = sin muestras. El tipo hace imposible comparar contra el piso
        // sin denominador, que era el defecto.
        if let Some(rate) = self
            .session_mgr
            .current()
            .agent
            .session_token_tracker
            .cache_hit_rate()
        {
            if rate < CACHE_HIT_RATE_FLOOR {
                let sid = self.session_mgr.current().metadata.session_id.to_string();
                let tracker = &self.session_mgr.current().agent.session_token_tracker;
                tracing::warn!(
                    input = tracker.total_input_tokens,
                    cache_read = tracker.total_cache_read_tokens,
                    rate_pct = rate * 100.0,
                    "prompt cache hit rate below threshold"
                );
                nexum_agent::metrics::emit(
                    "trap.cache_anomaly",
                    serde_json::json!({
                        "rate": rate,
                        "threshold": CACHE_HIT_RATE_FLOOR,
                        "request_id": tracker.last_request_id.as_deref().unwrap_or("-"),
                        "total_input_tokens": tracker.total_input_tokens,
                        "total_cache_read_tokens": tracker.total_cache_read_tokens,
                    }),
                    Some(&sid),
                    None,
                );
            }
        }
        // 更新 spinner 的 token 显示（仅当次调用的 token，不累计）
        let current_tokens = usage.input_tokens as usize + usage.output_tokens as usize;
        self.session_mgr
            .current_mut()
            .spinner_state
            .set_token_count(current_tokens);
        (true, false, false)
    }

    pub(super) fn handle_subagent_start(
        &mut self,
        agent_id: String,
        instance_id: String,
        task_preview: String,
        is_background: bool,
    ) -> (bool, bool, bool) {
        if is_background {
            use super::super::chat_session::RunningBgAgent;
            self.session_mgr
                .current_mut()
                .background_agents
                .push(RunningBgAgent {
                    agent_name: agent_id.clone(),
                    instance_id: instance_id.clone(),
                    started_at: std::time::Instant::now(),
                    tool_count: 0,
                });
        }
        self.session_mgr.current_mut().agent.subagent_depth += 1;
        // Pipeline：创建 SubAgentGroup VM
        let actions = self
            .session_mgr
            .current_mut()
            .messages
            .pipeline
            .handle_event(AgentEvent::SubAgentStart {
                agent_id,
                instance_id,
                task_preview,
                is_background,
            });
        for action in actions {
            self.apply_pipeline_action(action);
        }
        self.request_rebuild();
        (true, false, false)
    }
}
