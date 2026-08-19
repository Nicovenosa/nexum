//! Transport abstraction for ACP JSON-RPC 2.0 bidirectional communication.
//!
//! The [`AcpTransport`] trait provides a unified interface for sending requests,
//! notifications, and responses, and for receiving incoming messages. Two
//! implementations are provided:
//!
//! - [`MpscTransport`](mpsc) — in-memory channel pair for TUI ↔ ACP Server
//! - [`StdioTransport`](stdio) — stdio-based transport for external IDE clients

pub mod hub;
pub mod local;
pub mod mpsc;
pub mod stdio;
pub mod types;
#[cfg(unix)]
pub mod unix;

#[cfg(all(test, unix))]
mod hub_test;

#[cfg(all(test, unix))]
mod unix_test;

#[cfg(test)]
mod local_test;

use async_trait::async_trait;
use serde_json::Value;
use types::{AcpError, IncomingMessage, RequestId};

/// Bidirectional ACP JSON-RPC 2.0 transport.
///
/// Implementations are responsible for serializing/deserializing messages
/// to/from the underlying transport (mpsc channels, stdio, WebSocket, etc.).
#[async_trait]
pub trait AcpTransport: Send + Sync {
    /// Send a request and wait for a response.
    async fn send_request(&self, method: &str, params: Value) -> Result<Value, AcpError>;

    /// Send a notification (fire-and-forget, no response expected).
    async fn send_notification(&self, method: &str, params: Value) -> Result<(), AcpError>;

    /// Receive the next incoming message, or `None` if the transport is closed.
    async fn recv(&self) -> Option<IncomingMessage>;

    /// Send a response to a previously-received request.
    async fn send_response(
        &self,
        id: RequestId,
        result: Result<Value, AcpError>,
    ) -> Result<(), AcpError>;

    /// Cierra el canal del cliente sin implicar ownership del proceso remoto.
    /// Los transports sin recursos externos pueden conservar el no-op seguro.
    async fn close(&self) -> Result<(), AcpError> {
        Ok(())
    }
}
