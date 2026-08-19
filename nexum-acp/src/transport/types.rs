//! Core transport types for ACP JSON-RPC 2.0 communication.

/// Versión del framing local. Debe cambiar si cambia el encabezado binario.
/// Portable: lo consume el servidor ACP en todas las plataformas.
pub const LOCAL_PROTOCOL_VERSION: u16 = 1;

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request/response identifier.
///
/// Mirrors the ACP spec: can be a string or number.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(i64),
}

/// Principal durable autenticado por el host. El transporte nunca lo infiere
/// desde parámetros ACP controlados por el cliente.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HostPrincipal(String);

impl HostPrincipal {
    pub fn new(value: impl Into<String>) -> Result<Self, AcpError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AcpError::new(-32602, "host principal cannot be empty"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Contexto efímero adjuntado por un multiplexor autenticado al request actual.
///
/// El identificador de conexión nunca debe persistirse ni tratarse como una
/// identidad durable: sólo permite que el host verifique la suscripción vigente
/// mientras procesa este request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerContext {
    connection_id: u64,
    principal: Option<HostPrincipal>,
}

impl CallerContext {
    pub(crate) fn from_connection(connection_id: u64, principal: Option<HostPrincipal>) -> Self {
        Self {
            connection_id,
            principal,
        }
    }

    pub(crate) fn belongs_to_connection(&self, connection_id: u64) -> bool {
        self.connection_id == connection_id
    }

    pub fn principal(&self) -> Option<&HostPrincipal> {
        self.principal.as_ref()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestId::String(s) => write!(f, "{s}"),
            RequestId::Number(n) => write!(f, "{n}"),
        }
    }
}

/// An incoming JSON-RPC 2.0 message from the transport.
#[derive(Debug)]
pub enum IncomingMessage {
    /// A request that expects a response.
    Request {
        id: RequestId,
        method: String,
        params: Value,
        caller: Option<CallerContext>,
    },
    /// A notification that does not expect a response.
    Notification { method: String, params: Value },
    /// A response to a previous request.
    Response {
        id: RequestId,
        result: Result<Value, AcpError>,
    },
}

/// ACP transport-level error, compatible with JSON-RPC 2.0 error objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl AcpError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

impl fmt::Display for AcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ACP error [{}]: {}", self.code, self.message)
    }
}

impl std::error::Error for AcpError {}
