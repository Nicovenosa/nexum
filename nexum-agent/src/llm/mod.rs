pub mod anthropic;
pub mod openai;
pub mod retry;
pub mod sse;
pub mod tool_call_recovery;
pub mod types;

mod adapter;
mod react_adapter;

// Re-export types for external crate usage (e.g. BaseModel trait impls, tests)
use async_trait::async_trait;

pub use self::react_adapter::BaseModelReactLLM;
pub use self::retry::{RetryConfig, RetryableLLM};
pub use self::types::{LlmRequest, LlmResponse, StopReason, StreamingContext};
use crate::error::AgentResult;

/// BaseModel trait - 统一 LLM 接口，对齐 LangChain Python BaseModel
#[async_trait]
pub trait BaseModel: Send + Sync {
    async fn invoke(&self, request: LlmRequest) -> AgentResult<LlmResponse>;
    fn provider_name(&self) -> &str;
    fn model_id(&self) -> &str;

    /// 模型的上下文窗口大小（token 数）
    ///
    /// 用于 token 用量追踪和上下文压缩决策。
    /// 默认返回 200_000（适用于大多数 modern LLM）。
    fn context_window(&self) -> u32 {
        200_000
    }

    /// 是否原生支持流式调用。
    ///
    /// Capability Query：调用方据此决定是直接请求 SSE 流，
    /// 还是要走 [`invoke_streaming`](Self::invoke_streaming) 默认实现的
    /// "invoke + 一次性返回" 降级路径。默认 `false`，
    /// 由 [`ChatOpenAI`] / [`ChatAnthropic`] override 返回 `true`。
    fn supports_streaming(&self) -> bool {
        false
    }

    /// 流式调用。默认实现回退到非流式 invoke()。
    /// 仅 ChatOpenAI 和 ChatAnthropic override 此方法实现 SSE 流式。
    async fn invoke_streaming(
        &self,
        request: LlmRequest,
        _ctx: StreamingContext,
    ) -> AgentResult<LlmResponse> {
        tracing::debug!(
            provider = self.provider_name(),
            model = self.model_id(),
            "LLM 未声明 supports_streaming，invoke_streaming 降级为非流式 invoke"
        );
        self.invoke(request).await
    }
}

pub use adapter::MockLLM;
pub use anthropic::ChatAnthropic;
pub use openai::ChatOpenAI;

/// Build a reqwest client with connection pool limits to prevent TLS session
/// accumulation. Default pool is unbounded — each idle connection holds
/// ~50-100 KB of TLS state that is never released.
/// Connect timeout duro: acota el establecimiento de conexión (SYN/TLS).
const CONNECT_TIMEOUT_SECS: u64 = 10;
/// Read timeout por defecto (SPEC-PROVIDERS-001 GAP-P1): tiempo máximo de
/// silencio entre bytes. Un provider que acepta y no responde dispara acá,
/// SIN cortar streaming legítimo (cada byte recibido reinicia el reloj). El
/// default (< 30 s) cumple el gate de release; override consciente permitido
/// con máximo duro.
const READ_TIMEOUT_DEFAULT_SECS: u64 = 25;
const READ_TIMEOUT_MAX_SECS: u64 = 300;
/// Budget de read-timeout para providers LOCALES de CPU (p. ej. Ollama).
///
/// Causa raíz (repro FASE 1, micro-fix Ollama): Ollama NO envía los headers
/// HTTP de `/v1/chat/completions` hasta terminar el **prefill**
/// (time-to-first-token) del modelo; en modo non-stream retiene además toda la
/// generación. Con un input grande en CPU ese silencio supera el read-timeout
/// remoto de 25 s y `.send().await` corta con `is_timeout` ("error sending
/// request … operation timed out") a exactamente 25 s.
///
/// Un provider LOCAL de CPU legítimamente no emite bytes durante el prefill:
/// ese silencio NO es un cuelgue. Este budget capability-aware tolera la
/// latencia local **sin** relajar la garantía de los providers remotos (que
/// deben streamear y donde 25 s de silencio sí indican cuelgue). Bounded por
/// `READ_TIMEOUT_MAX_SECS`, tunable por env. NO es un aumento global ciego:
/// se aplica sólo a providers marcados como locales por la capa de provider.
const LOCAL_READ_TIMEOUT_DEFAULT_SECS: u64 = 120;

fn read_timeout() -> std::time::Duration {
    let secs = std::env::var("NEXUM_PROVIDER_READ_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|v| v.clamp(1, READ_TIMEOUT_MAX_SECS)) // impide "sin límite"; permite menor
        .unwrap_or(READ_TIMEOUT_DEFAULT_SECS);
    std::time::Duration::from_secs(secs)
}

/// Read-timeout para providers LOCALES de CPU (capability-aware). Ver
/// [`LOCAL_READ_TIMEOUT_DEFAULT_SECS`]. Tunable con
/// `NEXUM_LOCAL_PROVIDER_READ_TIMEOUT_SECS`, clamped a `[1, READ_TIMEOUT_MAX_SECS]`.
pub(crate) fn local_read_timeout() -> std::time::Duration {
    let secs = std::env::var("NEXUM_LOCAL_PROVIDER_READ_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|v| v.clamp(1, READ_TIMEOUT_MAX_SECS))
        .unwrap_or(LOCAL_READ_TIMEOUT_DEFAULT_SECS);
    std::time::Duration::from_secs(secs)
}

pub(crate) fn build_reqwest_client() -> reqwest::Client {
    build_reqwest_client_with_read_timeout(read_timeout())
}

/// Igual que [`build_reqwest_client`] pero con un read-timeout explícito.
/// El connect-timeout (SYN/TLS) se mantiene fijo y distinto del read-timeout,
/// para diferenciar "no conecta" de "conecta y tarda en generar".
pub(crate) fn build_reqwest_client_with_read_timeout(rt: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        // Sin User-Agent, Cloudflare devuelve 403 en algunos providers
        // (verificado contra opencode.ai/zen — fix UX popups Bug 2).
        .user_agent(concat!("nexum/", env!("CARGO_PKG_VERSION"), " (nexum-tui)"))
        .pool_max_idle_per_host(1)
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        // GAP-P1: ningún provider puede colgar el runtime indefinidamente.
        .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .read_timeout(rt)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(test)]
#[path = "ollama_repro_test.rs"]
mod ollama_repro_test;

#[cfg(test)]
mod client_timeout_test {
    use super::*;

    /// PST-2 GAP-P1: un provider que acepta TCP y nunca responde debe cortar
    /// por read_timeout, no colgar. Con override corto para no demorar el test.
    #[tokio::test]
    async fn test_read_timeout_corta_provider_colgado() {
        use std::io::Read;
        std::env::set_var("NEXUM_PROVIDER_READ_TIMEOUT_SECS", "2");
        // Socket que acepta y nunca responde.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                // Aceptar y quedarse leyendo sin responder jamás.
                std::thread::spawn(move || {
                    let mut s = stream.unwrap();
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf);
                    std::thread::sleep(std::time::Duration::from_secs(60));
                });
            }
        });
        let client = build_reqwest_client();
        std::env::remove_var("NEXUM_PROVIDER_READ_TIMEOUT_SECS");
        let t0 = std::time::Instant::now();
        let res = client
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .body("{}")
            .send()
            .await;
        let elapsed = t0.elapsed();
        assert!(res.is_err(), "el request debe fallar por timeout, no colgar");
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "read_timeout corta el silencio del provider: {elapsed:?}"
        );
    }

    #[test]
    fn test_read_timeout_config_clamp() {
        std::env::set_var("NEXUM_PROVIDER_READ_TIMEOUT_SECS", "99999");
        assert_eq!(read_timeout().as_secs(), READ_TIMEOUT_MAX_SECS, "máximo duro");
        std::env::set_var("NEXUM_PROVIDER_READ_TIMEOUT_SECS", "0");
        assert_eq!(read_timeout().as_secs(), 1, "mínimo 1, impide 0/sin límite");
        std::env::set_var("NEXUM_PROVIDER_READ_TIMEOUT_SECS", "5");
        assert_eq!(read_timeout().as_secs(), 5, "permite un valor menor");
        std::env::remove_var("NEXUM_PROVIDER_READ_TIMEOUT_SECS");
        assert_eq!(read_timeout().as_secs(), READ_TIMEOUT_DEFAULT_SECS, "default seguro");
    }
}
