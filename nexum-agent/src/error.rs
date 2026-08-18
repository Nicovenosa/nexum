use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentError {
    #[error("Max iterations exceeded ({0})")]
    MaxIterationsExceeded(usize),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Tool execution failed: {tool} - {reason}")]
    ToolExecutionFailed { tool: String, reason: String },

    #[error("LLM error: {0}")]
    LlmError(String),

    #[error(
        "LLM_TRANSPORT_FAILURE class={classification} endpoint={endpoint} request_sent={request_sent}"
    )]
    LlmTransportError {
        classification: &'static str,
        endpoint: String,
        request_sent: bool,
    },

    #[error("LLM HTTP 错误 ({status}): {message}")]
    LlmHttpError { status: u16, message: String },

    #[error("Middleware error: {middleware} - {reason}")]
    MiddlewareError { middleware: String, reason: String },

    #[error("Tool rejected: {tool} - {reason}")]
    ToolRejected { tool: String, reason: String },

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// 用户主动中断（Ctrl+C）
    #[error("Interrupted by user")]
    Interrupted,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type AgentResult<T> = Result<T, AgentError>;

impl AgentError {
    pub fn from_reqwest(error: reqwest::Error) -> Self {
        use std::error::Error as _;

        let mut detail = error.to_string().to_lowercase();
        let mut source = error.source();
        while let Some(value) = source {
            detail.push(' ');
            detail.push_str(&value.to_string().to_lowercase());
            source = value.source();
        }

        let classification = if detail.contains("dns")
            || detail.contains("name resolution")
            || detail.contains("failed to lookup")
            || detail.contains("no address associated")
        {
            "DNS"
        } else if detail.contains("certificate")
            || detail.contains("tls")
            || detail.contains("handshake")
        {
            "TLS"
        } else if error.is_timeout() && error.is_connect() {
            "CONNECT_TIMEOUT"
        } else if error.is_timeout() {
            "READ_TIMEOUT"
        } else if detail.contains("connection reset") {
            "CONNECTION_RESET"
        } else if error.is_connect()
            || detail.contains("connection refused")
            || detail.contains("connection aborted")
        {
            "UPSTREAM_UNAVAILABLE"
        } else {
            "UNKNOWN"
        };
        let request_sent = !matches!(
            classification,
            "DNS" | "TLS" | "CONNECT_TIMEOUT" | "UPSTREAM_UNAVAILABLE"
        );
        let endpoint = error
            .url()
            .and_then(|url| url.host_str().map(|host| (url.scheme(), host)))
            .map(|(scheme, host)| format!("{scheme}://{host}"))
            .unwrap_or_else(|| "unknown".to_string());
        Self::LlmTransportError {
            classification,
            endpoint,
            request_sent,
        }
    }

    /// 判断错误是否可重试（用于 LLM 调用重试机制）
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::LlmHttpError { status, .. } => {
                matches!(status, 408 | 429 | 500..=599)
            }
            Self::LlmError(msg) => {
                let msg_lower = msg.to_lowercase();
                // GAP-P1: un silencio del provider (read/connect timeout) NO se
                // cura reintentando de inmediato — devolver error claro rápido
                // (el cliente ya acota cada intento por connect/read timeout).
                // Solo se reintentan fallos transitorios reales (bridge que
                // reinicia, reset de conexión, límite de tasa, sobrecarga).
                msg_lower.contains("connection refused")
                    || msg_lower.contains("connection reset")
                    || msg_lower.contains("connection aborted")
                    || msg_lower.contains("broken pipe")
                    || msg_lower.contains("dns")
                    || msg_lower.contains("rate limit")
                    || msg_lower.contains("overloaded")
            }
            Self::LlmTransportError { classification, .. } => {
                matches!(*classification, "CONNECTION_RESET" | "UPSTREAM_UNAVAILABLE")
            }
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "error_test.rs"]
mod tests;
