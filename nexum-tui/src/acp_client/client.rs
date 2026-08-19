//! Thin TUI-side wrapper around an object-safe ACP client transport.
//!
//! Translates raw [`IncomingMessage`]s into [`AcpNotification`]s for the TUI event
//! loop to consume. The notification pump runs as a background tokio task.

use std::sync::{Arc, Mutex};

use nexum_acp::transport::{
    types::{AcpError, IncomingMessage, RequestId},
    AcpTransport,
};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// Transporte consumido por la interfaz. Mantiene el cliente independiente de
/// MPSC, sockets Unix o la politica de bootstrap.
pub trait AcpClientTransport: AcpTransport {}

impl<T> AcpClientTransport for T where T: AcpTransport + ?Sized {}

/// Notification events dispatched from the background pump to the TUI event loop.
pub enum AcpNotification {
    /// A `notifications/agent_event` notification carrying a nexum-agent ExecutorEvent.
    /// The TUI converts this to its own AgentEvent via `map_executor_event`.
    AgentEvent {
        session_id: String,
        event: nexum_agent::agent::events::AgentEvent,
    },
    /// A `notifications/session_update` notification from the ACP server.
    SessionUpdate { session_id: String, params: Value },
    /// A `RequestPermission` request requiring HITL interaction.
    RequestPermission { id: RequestId, params: Value },
    /// An `elicitation/create` request requiring AskUser interaction.
    Elicitation { id: RequestId, params: Value },
    /// An unrecognized notification or request.
    Other { msg: String },
    /// Agent execution completed (synthetic notification from ACP server).
    AgentDone { session_id: String },
    /// The transport closed before an ACP terminal event was delivered.
    TurnFailed { message: String },
    /// Typed terminal emitted by the stable ACP runtime.
    TurnTerminal {
        state: nexum_acp::session::terminal::TerminalState,
        message: Option<String>,
    },
    /// Prediction fork 完成后的建议文本。
    PredictionReady { session_id: String, text: String },
    /// A `notifications/peri/*` custom notification (SubAgent, Compact, LSP, etc.)
    Peri {
        session_id: String,
        method: String,
        params: Value,
    },
}

/// TUI-side client that owns the ACP transport and routes notifications.
///
/// Uses `Arc<Mutex<Option<String>>>` for `current_session_id` so that
/// clones (e.g., in `interrupt()` and `submit_message()`'s async task)
/// share the same session state.
#[derive(Clone)]
pub struct AcpTuiClient {
    transport: Arc<dyn AcpClientTransport>,
    notification_tx: mpsc::UnboundedSender<AcpNotification>,
    current_session_id: Arc<Mutex<Option<String>>>,
}

impl AcpTuiClient {
    /// Check whether a session has been created.
    pub fn has_session(&self) -> bool {
        self.current_session_id.lock().unwrap().is_some()
    }

    pub fn current_session_id(&self) -> Option<String> {
        self.current_session_id.lock().unwrap().clone()
    }

    /// Create a new client wrapping an existing ACP client transport.
    ///
    /// Returns `(Self, notification_receiver)`. The caller must:
    /// 1. Move `notification_receiver` to the TUI event loop (`AgentComm.acp_notification_rx`)
    /// 2. Spawn the pump via [`AcpTuiClient::spawn_pump`]
    pub fn new(
        transport: Arc<dyn AcpClientTransport>,
    ) -> (Self, mpsc::UnboundedReceiver<AcpNotification>) {
        let (notification_tx, notification_rx) = mpsc::unbounded_channel();
        let client = Self {
            transport,
            notification_tx,
            current_session_id: Arc::new(Mutex::new(None)),
        };
        (client, notification_rx)
    }

    /// Spawn the notification pump as a tokio task. Consumes internal clones of
    /// transport and notification sender.
    pub fn spawn_pump(&self) {
        let transport = self.transport.clone();
        let notification_tx = self.notification_tx.clone();
        tokio::spawn(async move {
            Self::run_pump(transport, notification_tx).await;
        });
    }

    /// Desconecta este cliente ACP; no tiene ownership sobre un host local.
    pub async fn close(&self) -> Result<(), AcpError> {
        self.transport.close().await
    }

    /// Expone una request ACP sin imponer semántica de sesión de la TUI.
    /// Clientes especializados, como voz headless, siguen reutilizando este
    /// transporte y su pump de notificaciones sin heredar una sesión global.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        self.transport.send_request(method, params).await
    }

    /// Envía una notificación ACP sin estado local de TUI.
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), AcpError> {
        self.transport.send_notification(method, params).await
    }

    // ── Pump ──

    /// Background task that polls the transport and dispatches notifications.
    async fn run_pump(
        transport: Arc<dyn AcpClientTransport>,
        notification_tx: mpsc::UnboundedSender<AcpNotification>,
    ) {
        let mut event_count: u64 = 0;
        let mut structured_transport_failure_seen = false;
        loop {
            let msg = transport.recv().await;
            match msg {
                Some(IncomingMessage::Notification { method, params }) => {
                    if method == nexum_acp::transport::socket::TRANSPORT_CLOSED_METHOD {
                        structured_transport_failure_seen = true;
                        let classification = params
                            .get("classification")
                            .and_then(Value::as_str)
                            .unwrap_or("UNKNOWN");
                        let reason_code = params
                            .get("reason_code")
                            .and_then(Value::as_str)
                            .unwrap_or("ACP_TRANSPORT_UNKNOWN");
                        let detail = params
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("local ACP transport ended");
                        let _ = notification_tx.send(AcpNotification::TurnFailed {
                            message: format!(
                                "ACP terminal failure [{classification}/{reason_code}]: {detail}"
                            ),
                        });
                    } else if method == "peri/agent_event" {
                        event_count += 1;
                        let session_id = params
                            .get("sessionId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        // Prefer pre-serialized string (avoids clone + double-deserialize).
                        // Fall back to old "event" Value field for backward compat during rollout.
                        let event_result = if let Some(event_str) =
                            params.get("event_json").and_then(|v| v.as_str())
                        {
                            serde_json::from_str::<nexum_agent::agent::events::AgentEvent>(
                                event_str,
                            )
                        } else if let Some(event_value) = params.get("event") {
                            serde_json::from_value::<nexum_agent::agent::events::AgentEvent>(
                                event_value.clone(),
                            )
                        } else {
                            warn!("ACP client pump: agent_event notification missing 'event_json' or 'event' field");
                            continue;
                        };
                        match event_result {
                            Ok(event) => {
                                debug!(
                                    event_count = event_count,
                                    session_id = %session_id,
                                    "ACP client pump: received agent_event"
                                );
                                if matches!(
                                    &event,
                                    nexum_agent::agent::events::AgentEvent::BackgroundTaskCompleted(
                                        _
                                    )
                                ) {
                                    tracing::info!(
                                        event_count = event_count,
                                        "[bg-diag] client-pump: deserialized BackgroundTaskCompleted, sending to TUI"
                                    );
                                }
                                let _ = notification_tx
                                    .send(AcpNotification::AgentEvent { session_id, event });
                            }
                            Err(e) => {
                                error!(
                                    event_count = event_count,
                                    error = %e,
                                    "ACP client pump: failed to parse AgentEvent — event LOST"
                                );
                                let _ = notification_tx.send(AcpNotification::Other {
                                    msg: format!("failed to parse AgentEvent: {e}"),
                                });
                            }
                        }
                    } else if method == "session/update" {
                        let session_id = params
                            .get("sessionId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let _ = notification_tx
                            .send(AcpNotification::SessionUpdate { session_id, params });
                    } else if method == "peri/agent_event_done" {
                        let session_id = params
                            .get("sessionId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        debug!(
                            session_id = %session_id,
                            total_events = event_count,
                            "ACP client pump: received agent_event_done"
                        );
                        let _ = notification_tx.send(AcpNotification::AgentDone { session_id });
                    } else if method == "peri/turn_terminal" {
                        let state = params
                            .get("state")
                            .cloned()
                            .and_then(|value| serde_json::from_value(value).ok());
                        let message = params
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        match state {
                            Some(state) => {
                                let _ = notification_tx
                                    .send(AcpNotification::TurnTerminal { state, message });
                            }
                            None => {
                                let _ = notification_tx.send(AcpNotification::TurnFailed {
                                    message: "ACP sent an invalid stable terminal state"
                                        .to_string(),
                                });
                            }
                        }
                    } else if method == "peri/prediction_ready" {
                        let session_id = params
                            .get("sessionId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let text = params
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !text.is_empty() {
                            let _ = notification_tx
                                .send(AcpNotification::PredictionReady { session_id, text });
                        }
                    } else if method.starts_with("notifications/peri/") {
                        let session_id = params
                            .get("sessionId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let _ = notification_tx.send(AcpNotification::Peri {
                            session_id,
                            method,
                            params,
                        });
                    } else {
                        let _ = notification_tx.send(AcpNotification::Other {
                            msg: format!("notification: {method}"),
                        });
                    }
                }
                Some(IncomingMessage::Request {
                    id, method, params, ..
                }) => {
                    if method == "session/request_permission" {
                        let _ =
                            notification_tx.send(AcpNotification::RequestPermission { id, params });
                    } else if method == "elicitation/create" {
                        let _ = notification_tx.send(AcpNotification::Elicitation { id, params });
                    } else {
                        let _ = notification_tx.send(AcpNotification::Other {
                            msg: format!("request: {method}"),
                        });
                    }
                }
                Some(IncomingMessage::Response { .. }) => {}
                None => {
                    debug!("ACP client pump: transport closed, exiting");
                    if !structured_transport_failure_seen {
                        let _ = notification_tx.send(AcpNotification::TurnFailed {
                            message: "ACP terminal failure [UNKNOWN/ACP_CHANNEL_CLOSED]: \
                                      transport channel ended without a structured reason"
                                .to_string(),
                        });
                    }
                    break;
                }
            }
        }
    }

    // ── High-level RPC wrappers ──

    /// Create a new agent session.
    ///
    /// Closes the previous session (if any) to release its history, AgentPool,
    /// and FrozenSessionData from the server-side sessions HashMap.
    pub async fn new_session(&self, cwd: &str, model: Option<&str>) -> Result<String, AcpError> {
        // Close previous session to free server-side memory
        let old_id = self.current_session_id.lock().unwrap().take();
        if let Some(ref old_sid) = old_id {
            let params = json!({ "sessionId": old_sid });
            if let Err(e) = self.transport.send_request("session/close", params).await {
                debug!(error = %e, "Failed to close previous session (non-fatal)");
            }
        }

        let params = json!({ "cwd": cwd, "model": model });
        let result = self.transport.send_request("session/new", params).await?;
        // ACP protocol uses camelCase: {"sessionId": "..."}
        let session_id = result
            .get("sessionId")
            .or_else(|| result.get("session_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AcpError::new(-32603, "no session_id in response"))?
            .to_string();
        *self.current_session_id.lock().unwrap() = Some(session_id.clone());
        Ok(session_id)
    }

    /// Load an existing session from ThreadStore history.
    /// Used when restoring a historical thread so the ACP server has the full context.
    ///
    /// Closes the previous session (if any) to release server-side memory.
    pub async fn load_session(
        &self,
        session_id: &str,
        cwd: &str,
        model: Option<&str>,
    ) -> Result<String, AcpError> {
        // Close previous session (if different from the one being loaded)
        let old_id = self.current_session_id.lock().unwrap().take();
        if let Some(ref old_sid) = old_id {
            if old_sid != session_id {
                let params = json!({ "sessionId": old_sid });
                if let Err(e) = self.transport.send_request("session/close", params).await {
                    debug!(error = %e, "Failed to close previous session (non-fatal)");
                }
            }
        }

        let params = json!({ "sessionId": session_id, "cwd": cwd, "model": model });
        let _ = self.transport.send_request("session/load", params).await?;
        *self.current_session_id.lock().unwrap() = Some(session_id.to_string());
        Ok(session_id.to_string())
    }

    /// Submit a user message to the current session.
    /// Note: prompt() is called from the spawned async task that already
    /// has a session via new_session(), so current_session_id is guaranteed Some.
    pub async fn prompt(
        &self,
        content: &nexum_agent::messages::MessageContent,
    ) -> Result<(), AcpError> {
        let session_id = self
            .current_session_id
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AcpError::new(-32603, "no active session"))?;
        let params = json!({
            "sessionId": session_id,
            "message": { "role": "user", "content": content },
        });
        self.transport
            .send_request("session/prompt", params)
            .await
            .map(|_| ())
    }

    /// Stable TUI submission. Unlike `prompt`, this method cannot omit the
    /// typed task contract and marks the RPC so the server fails closed if the
    /// envelope is lost or invalid.
    pub async fn prompt_with_task_envelope(
        &self,
        content: &nexum_agent::messages::MessageContent,
        task_envelope: &nexum_acp::task::TaskEnvelopeV1,
    ) -> Result<(), AcpError> {
        let session_id = self
            .current_session_id
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AcpError::new(-32603, "no active session"))?;
        let params = json!({
            "sessionId": session_id,
            "message": { "role": "user", "content": content },
            "taskEnvelope": task_envelope,
            "stableProfile": true,
        });
        self.transport
            .send_request("session/prompt", params)
            .await
            .map(|_| ())
    }

    /// Converts background RPC/bootstrap failures into the same visible
    /// terminal channel consumed by the TUI.
    pub fn report_turn_failure(&self, message: impl Into<String>) {
        let _ = self.notification_tx.send(AcpNotification::TurnFailed {
            message: message.into(),
        });
    }

    /// Submit background task results as synthetic tool_use + tool_result pairs.
    /// The executor injects AgentResult tool calls with the results before the user message.
    pub async fn prompt_with_bg_results(
        &self,
        bg_results: Vec<nexum_agent::agent::events::BackgroundTaskResult>,
    ) -> Result<(), AcpError> {
        let session_id = self
            .current_session_id
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AcpError::new(-32603, "no active session"))?;
        let params = json!({
            "sessionId": session_id,
            "message": { "role": "user", "content": "Background agents completed. Please review the results." },
            "bgResults": bg_results,
        });
        self.transport
            .send_request("session/prompt", params)
            .await
            .map(|_| ())
    }

    /// Change the model for the current session.
    pub async fn set_model(&self, alias: &str) -> Result<(), AcpError> {
        let session_id = self
            .current_session_id
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AcpError::new(-32603, "no active session"))?;
        let params = json!({ "sessionId": session_id, "modelId": alias });
        let _ = self
            .transport
            .send_request("session/set_model", params)
            .await?;
        Ok(())
    }

    /// Change the permission mode for the current session.
    pub async fn set_mode(&self, mode: &str) -> Result<(), AcpError> {
        let session_id = self
            .current_session_id
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AcpError::new(-32603, "no active session"))?;
        let params = json!({ "sessionId": session_id, "modeId": mode });
        let _ = self
            .transport
            .send_request("session/set_mode", params)
            .await?;
        Ok(())
    }

    /// Set a config option (mode/model/thought_level) via the unified config API.
    /// Silently returns Ok if no session exists yet — uses notification to
    /// update ACP server state directly without requiring a session.
    pub async fn set_config_option(&self, config_id: &str, value: &str) -> Result<(), AcpError> {
        let session_id = {
            let guard = self.current_session_id.lock().unwrap();
            guard.clone()
        };
        match session_id {
            Some(session_id) => {
                let params =
                    json!({ "sessionId": session_id, "configId": config_id, "value": value });
                let _ = self
                    .transport
                    .send_request("session/set_config_option", params)
                    .await?;
            }
            None => {
                // No session yet — send via notification so ACP server updates its
                // nexum_config/provider before any session is created.
                let params = json!({ "configId": config_id, "value": value });
                self.transport
                    .send_notification("session/config_update", params)
                    .await?;
            }
        }
        Ok(())
    }

    /// Update the full NexumConfig on the ACP server (for Login panel CRUD).
    /// When no session exists, uses notification to update server state directly.
    pub async fn update_config(&self, config: &crate::config::NexumConfig) -> Result<(), AcpError> {
        let session_id = {
            let guard = self.current_session_id.lock().unwrap();
            guard.clone()
        };
        match session_id {
            Some(session_id) => {
                let params = json!({
                    "sessionId": session_id,
                    "config": config,
                });
                let _ = self
                    .transport
                    .send_request("session/update_config", params)
                    .await?;
            }
            None => {
                // No session yet — send via notification so ACP server updates
                // nexum_config/provider before any session is created.
                tracing::info!("update_config: no session, sending via notification");
                let params = json!({
                    "config": config,
                });
                self.transport
                    .send_notification("session/config_update", params)
                    .await?;
            }
        }
        Ok(())
    }

    /// Cancel the currently running prompt.
    pub async fn cancel(&self) -> Result<(), AcpError> {
        let session_id = self
            .current_session_id
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AcpError::new(-32603, "no active session"))?;
        let params = json!({ "sessionId": session_id });
        self.transport
            .send_notification("session/cancel", params)
            .await
    }

    /// Send a response to a server-initiated request (e.g. HITL approval).
    pub async fn send_response(
        &self,
        id: RequestId,
        result: Result<Value, AcpError>,
    ) -> Result<(), AcpError> {
        self.transport.send_response(id, result).await
    }
}
