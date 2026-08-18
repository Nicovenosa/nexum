//! Multiplexor de clientes ACP sobre un único servidor lógico.
//!
//! No interpreta métodos ACP: conserva los mensajes y sólo mantiene el routing
//! de conexión necesario para respuestas, eventos de sesión y solicitudes HITL.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};

use super::{
    types::{AcpError, CallerContext, HostPrincipal, IncomingMessage, RequestId},
    AcpTransport,
};

const HUB_QUEUE: usize = 256;

#[derive(Clone)]
struct Connection {
    id: u64,
    principal: Option<HostPrincipal>,
    transport: Arc<dyn AcpTransport>,
}

struct ResponseRoute {
    connection: Connection,
    original_id: RequestId,
    method: String,
    requested_session_id: Option<String>,
}

/// Unifica varios transports cliente en un único [`AcpTransport`] servidor.
pub struct AcpHub {
    incoming_tx: mpsc::Sender<IncomingMessage>,
    incoming_rx: Mutex<mpsc::Receiver<IncomingMessage>>,
    response_routes: Mutex<HashMap<RequestId, ResponseRoute>>,
    session_routes: Mutex<HashMap<String, Connection>>,
    next_connection_id: AtomicU64,
    active_connections: AtomicUsize,
    max_connections: usize,
}

impl AcpHub {
    pub fn new(max_connections: usize) -> Self {
        let (incoming_tx, incoming_rx) = mpsc::channel(HUB_QUEUE);
        Self {
            incoming_tx,
            incoming_rx: Mutex::new(incoming_rx),
            response_routes: Mutex::new(HashMap::new()),
            session_routes: Mutex::new(HashMap::new()),
            next_connection_id: AtomicU64::new(1),
            active_connections: AtomicUsize::new(0),
            max_connections,
        }
    }

    pub fn attach(self: &Arc<Self>, transport: Arc<dyn AcpTransport>) -> Result<(), AcpError> {
        self.attach_inner(transport, None)
    }

    /// Attaches a transport whose durable principal was authenticated by the
    /// host. The principal is distinct from the ephemeral connection ID.
    pub fn attach_with_principal(
        self: &Arc<Self>,
        transport: Arc<dyn AcpTransport>,
        principal: HostPrincipal,
    ) -> Result<(), AcpError> {
        self.attach_inner(transport, Some(principal))
    }

    fn attach_inner(
        self: &Arc<Self>,
        transport: Arc<dyn AcpTransport>,
        principal: Option<HostPrincipal>,
    ) -> Result<(), AcpError> {
        let count = self.active_connections.fetch_add(1, Ordering::AcqRel);
        if count >= self.max_connections {
            self.active_connections.fetch_sub(1, Ordering::AcqRel);
            return Err(AcpError::new(-32603, "Local ACP client limit reached"));
        }
        let connection = Connection {
            id: self.next_connection_id.fetch_add(1, Ordering::Relaxed),
            principal,
            transport,
        };
        let hub = Arc::clone(self);
        tokio::spawn(async move { hub.forward_connection(connection).await });
        Ok(())
    }

    async fn forward_connection(self: Arc<Self>, connection: Connection) {
        while let Some(message) = connection.transport.recv().await {
            let message = match message {
                IncomingMessage::Request {
                    id, method, params, ..
                } => {
                    let hub_id = RequestId::String(format!("hub:{}:{}", connection.id, id));
                    self.response_routes.lock().await.insert(
                        hub_id.clone(),
                        ResponseRoute {
                            connection: connection.clone(),
                            original_id: id,
                            method: method.clone(),
                            requested_session_id: session_subscription_target(&method, &params),
                        },
                    );
                    IncomingMessage::Request {
                        id: hub_id,
                        method,
                        params,
                        caller: Some(CallerContext::from_connection(
                            connection.id,
                            connection.principal.clone(),
                        )),
                    }
                }
                IncomingMessage::Notification { method, params } => {
                    IncomingMessage::Notification { method, params }
                }
                IncomingMessage::Response { .. } => continue,
            };
            if self.incoming_tx.send(message).await.is_err() {
                break;
            }
        }
        self.remove_connection(connection.id).await;
        self.active_connections.fetch_sub(1, Ordering::AcqRel);
    }

    async fn remove_connection(&self, connection_id: u64) {
        self.response_routes
            .lock()
            .await
            .retain(|_, route| route.connection.id != connection_id);
        self.session_routes
            .lock()
            .await
            .retain(|_, connection| connection.id != connection_id);
    }

    async fn route_session(&self, params: &Value) -> Result<Connection, AcpError> {
        let session_id = session_id(params)
            .ok_or_else(|| AcpError::new(-32602, "missing sessionId for local client routing"))?;
        self.session_routes
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| AcpError::new(-32602, "session has no local client route"))
    }

    /// Comprueba la suscripción vigente de un caller para una sesión. El
    /// `CallerContext` sólo vive durante el request y no se persiste.
    pub async fn caller_owns_session(&self, caller: &CallerContext, session_id: &str) -> bool {
        self.session_routes
            .lock()
            .await
            .get(session_id)
            .is_some_and(|connection| caller.belongs_to_connection(connection.id))
    }
}

#[async_trait]
impl AcpTransport for AcpHub {
    async fn send_request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        self.route_session(&params)
            .await?
            .transport
            .send_request(method, params)
            .await
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<(), AcpError> {
        self.route_session(&params)
            .await?
            .transport
            .send_notification(method, params)
            .await
    }

    async fn recv(&self) -> Option<IncomingMessage> {
        self.incoming_rx.lock().await.recv().await
    }

    async fn send_response(
        &self,
        id: RequestId,
        result: Result<Value, AcpError>,
    ) -> Result<(), AcpError> {
        let route = self
            .response_routes
            .lock()
            .await
            .remove(&id)
            .ok_or_else(|| AcpError::new(-32602, "unknown local response route"))?;
        if let Ok(value) = &result {
            let session_id = match route.method.as_str() {
                "session/new" | "session/fork" => session_id(value),
                "session/load" | "session/resume" => route.requested_session_id,
                _ => None,
            };
            if let Some(session_id) = session_id {
                let mut session_routes = self.session_routes.lock().await;
                if matches!(route.method.as_str(), "session/load" | "session/resume") {
                    // A reconnecting client must receive the resumed session's
                    // notifications instead of leaving them on a stale route.
                    session_routes.insert(session_id, route.connection.clone());
                } else {
                    session_routes
                        .entry(session_id)
                        .or_insert_with(|| route.connection.clone());
                }
            }
        }
        route
            .connection
            .transport
            .send_response(route.original_id, result)
            .await
    }
}

fn session_id(params: &Value) -> Option<String> {
    params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .or_else(|| params.pointer("/form/scope/sessionId"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn session_subscription_target(method: &str, params: &Value) -> Option<String> {
    matches!(method, "session/load" | "session/resume")
        .then(|| session_id(params))
        .flatten()
}
