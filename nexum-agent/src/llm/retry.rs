use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use rand::RngExt;

use crate::{
    agent::{
        events::{AgentEvent, AgentEventHandler},
        react::{ReactLLM, Reasoning},
    },
    error::AgentResult,
    messages::BaseMessage,
    tools::BaseTool,
};

/// 重试配置
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: usize,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            base_delay_ms: 500,
            max_delay_ms: 32_000,
        }
    }
}

impl RetryConfig {
    /// Stable one-shot execution never retries a provider request.
    pub fn one_shot() -> Self {
        Self::default().with_max_retries(0)
    }

    pub fn with_max_retries(mut self, n: usize) -> Self {
        self.max_retries = n;
        self
    }
    pub fn with_base_delay_ms(mut self, ms: u64) -> Self {
        self.base_delay_ms = ms;
        self
    }
    pub fn with_max_delay_ms(mut self, ms: u64) -> Self {
        self.max_delay_ms = ms;
        self
    }

    /// 指数退避 + 25% 随机抖动
    ///
    /// attempt 从 0 开始，但首次重试（attempt=0）使用 base_delay * 2
    /// 以确保对 429 限流有足够等待时间。
    pub fn exponential_delay(&self, attempt: usize) -> u64 {
        let effective = attempt + 1;
        let base =
            (self.base_delay_ms as f64 * 2f64.powi(effective as i32)).min(self.max_delay_ms as f64);
        let mut rng = rand::rng();
        let jitter = rng.random_range(0.0..0.25) * base;
        (base + jitter) as u64
    }
}

/// Budget total de reintentos (GAP-P1). Default 30 s (cumple el gate de
/// release); override consciente `NEXUM_PROVIDER_TOTAL_BUDGET_SECS` con máximo
/// duro para impedir un valor sin límite.
fn provider_total_budget() -> Duration {
    const DEFAULT_SECS: u64 = 30;
    const MAX_SECS: u64 = 600;
    let secs = std::env::var("NEXUM_PROVIDER_TOTAL_BUDGET_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|v| v.clamp(1, MAX_SECS))
        .unwrap_or(DEFAULT_SECS);
    Duration::from_secs(secs)
}

/// ReactLLM 装饰器：在调用失败时自动重试
pub struct RetryableLLM<L: ReactLLM> {
    inner: L,
    config: RetryConfig,
    event_handler: Option<Arc<dyn AgentEventHandler>>,
}

impl<L: ReactLLM> RetryableLLM<L> {
    pub fn new(inner: L, config: RetryConfig) -> Self {
        Self {
            inner,
            config,
            event_handler: None,
        }
    }

    pub fn with_event_handler(mut self, handler: Arc<dyn AgentEventHandler>) -> Self {
        self.event_handler = Some(handler);
        self
    }

    fn emit(&self, event: AgentEvent) {
        if let Some(h) = &self.event_handler {
            h.on_event(event);
        }
    }
}

#[async_trait]
impl<L: ReactLLM> ReactLLM for RetryableLLM<L> {
    async fn generate_reasoning(
        &self,
        messages: &[BaseMessage],
        tools: &[&dyn BaseTool],
        streaming: Option<crate::llm::types::StreamingContext>,
    ) -> AgentResult<Reasoning> {
        // GAP-P1: budget total del CONJUNTO de reintentos. Acota la suma de
        // intentos fallidos + backoff a un máximo duro, garantizando que un
        // provider indisponible termina con error claro dentro del budget
        // (gate < 30 s con el default). NO afecta un intento exitoso: un Ok
        // (incluido streaming largo) retorna de inmediato sin mirar el budget;
        // el budget solo se consulta ANTES de dormir para reintentar.
        let started = std::time::Instant::now();
        let total_budget = provider_total_budget();
        // 重试循环：attempt 0..max_retries，每次失败若可重试则延迟后继续
        for attempt in 0..self.config.max_retries {
            // 仅首次尝试透传 streaming，重试时传 None 防止同一 message_id 双重发射
            let retry_streaming = if attempt == 0 {
                streaming.clone()
            } else {
                None
            };
            match self
                .inner
                .generate_reasoning(messages, tools, retry_streaming)
                .await
            {
                Ok(r) => return Ok(r),
                Err(e) if e.is_retryable() => {
                    let delay = self.config.exponential_delay(attempt);
                    // Si el próximo delay excedería el budget total, abandonar
                    // ya con el error actual (no colgar reintentando).
                    if started.elapsed() + Duration::from_millis(delay) >= total_budget {
                        tracing::warn!(
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            budget_ms = total_budget.as_millis() as u64,
                            "LLM: budget total de reintentos agotado, error claro"
                        );
                        return Err(e);
                    }
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_retries = self.config.max_retries,
                        delay_ms = delay,
                        error = %e,
                        "LLM 调用失败，准备重试"
                    );
                    self.emit(AgentEvent::LlmRetrying {
                        attempt: attempt + 1,
                        max_attempts: self.config.max_retries,
                        delay_ms: delay,
                        error: e.to_string(),
                    });
                    crate::metrics::emit(
                        "llm.retry",
                        serde_json::json!({
                            "attempt": attempt + 1,
                            "max_attempts": self.config.max_retries,
                            "model": self.inner.model_name(),
                            "error": e.to_string(),
                            "delay_ms": delay,
                        }),
                        None,
                        None,
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                Err(e) => return Err(e),
            }
        }
        // 最终尝试（不重试），直接返回结果或错误（重试已耗尽，传 None 避免双重发射）
        self.inner.generate_reasoning(messages, tools, None).await
    }

    fn model_name(&self) -> String {
        self.inner.model_name()
    }

    fn context_window(&self) -> u32 {
        self.inner.context_window()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use super::*;
    use crate::error::AgentError;
    include!("retry_test.rs");
}
