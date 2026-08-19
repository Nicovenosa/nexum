//! Transporte ACP local sobre Unix domain sockets.
//!
//! El socket sólo transporta mensajes ACP serializados como JSON-RPC. El prefijo
//! binario agrega versión, correlación y límite de tamaño sin crear un protocolo
//! semántico alternativo.

use std::{
    collections::HashMap,
    io::ErrorKind,
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        unix::{OwnedReadHalf, OwnedWriteHalf},
        UnixStream,
    },
    sync::{mpsc, oneshot, Mutex},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use super::{
    types::{AcpError, IncomingMessage, RequestId},
    AcpTransport,
};
pub use super::types::LOCAL_PROTOCOL_VERSION;

/// Máximo por mensaje para impedir que un cliente local agote memoria.
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
const MAGIC: [u8; 4] = *b"NXAC";
const HEADER_BYTES: usize = 4 + 2 + 8 + 4;
const FRAME_BODY_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);
const INCOMING_QUEUE: usize = 64;
pub const TRANSPORT_CLOSED_METHOD: &str = "peri/transport_closed";

/// Frame local que envuelve un payload ACP JSON-RPC.
#[derive(Debug, Clone, PartialEq)]
pub struct WireFrame {
    pub protocol_version: u16,
    pub request_id: u64,
    pub payload: Value,
}

impl WireFrame {
    pub fn request(request_id: u64, payload: Value) -> Self {
        Self {
            protocol_version: LOCAL_PROTOCOL_VERSION,
            request_id,
            payload,
        }
    }
}

/// Errores de framing sin incluir payloads, tokens ni datos del cliente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    FrameTooLarge,
    IncompleteFrame,
    PeerClosed,
    ReadTimeout,
    IoFailure,
    InvalidMagic,
    IncompatibleProtocol,
    InvalidPayload,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::FrameTooLarge => "frame exceeds local transport limit",
            Self::IncompleteFrame => "incomplete local transport frame",
            Self::PeerClosed => "local transport peer closed",
            Self::ReadTimeout => "local transport frame body timed out",
            Self::IoFailure => "local transport I/O failed",
            Self::InvalidMagic => "invalid local transport frame",
            Self::IncompatibleProtocol => "incompatible local transport protocol",
            Self::InvalidPayload => "invalid ACP payload",
        };
        f.write_str(message)
    }
}

impl std::error::Error for FrameError {}

/// Codificador puro del encabezado y el payload JSON. Las operaciones I/O viven
/// en el host/transport y no tienen acceso a secretos de configuración.
pub struct FrameCodec;

impl FrameCodec {
    pub fn encode(frame: &WireFrame) -> Result<Vec<u8>, FrameError> {
        if frame.protocol_version != LOCAL_PROTOCOL_VERSION {
            return Err(FrameError::IncompatibleProtocol);
        }
        let payload = serde_json::to_vec(&frame.payload).map_err(|_| FrameError::InvalidPayload)?;
        if payload.len() > MAX_FRAME_BYTES {
            return Err(FrameError::FrameTooLarge);
        }

        let payload_len = u32::try_from(payload.len()).map_err(|_| FrameError::FrameTooLarge)?;
        let mut encoded = Vec::with_capacity(HEADER_BYTES + payload.len());
        encoded.extend_from_slice(&MAGIC);
        encoded.extend_from_slice(&frame.protocol_version.to_be_bytes());
        encoded.extend_from_slice(&frame.request_id.to_be_bytes());
        encoded.extend_from_slice(&payload_len.to_be_bytes());
        encoded.extend_from_slice(&payload);
        Ok(encoded)
    }

    pub fn decode(bytes: &[u8]) -> Result<WireFrame, FrameError> {
        if bytes.len() < HEADER_BYTES {
            return Err(FrameError::IncompleteFrame);
        }
        if bytes[..4] != MAGIC {
            return Err(FrameError::InvalidMagic);
        }
        let version = u16::from_be_bytes([bytes[4], bytes[5]]);
        if version != LOCAL_PROTOCOL_VERSION {
            return Err(FrameError::IncompatibleProtocol);
        }
        let request_id = u64::from_be_bytes(bytes[6..14].try_into().expect("fixed header"));
        let payload_len =
            u32::from_be_bytes(bytes[14..18].try_into().expect("fixed header")) as usize;
        if payload_len > MAX_FRAME_BYTES {
            return Err(FrameError::FrameTooLarge);
        }
        if bytes.len() != HEADER_BYTES + payload_len {
            return Err(FrameError::IncompleteFrame);
        }
        let payload = serde_json::from_slice(&bytes[HEADER_BYTES..])
            .map_err(|_| FrameError::InvalidPayload)?;
        Ok(WireFrame {
            protocol_version: version,
            request_id,
            payload,
        })
    }
}

type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, AcpError>>>>>;

/// Transporte ACP sobre un único stream Unix autenticado por el host.
///
/// Cada dirección tiene una cola acotada: un cliente lento recibe un error en
/// lugar de hacer crecer memoria sin límite.
pub struct UnixTransport {
    writer: Mutex<Option<OwnedWriteHalf>>,
    incoming: Mutex<mpsc::Receiver<IncomingMessage>>,
    pending: PendingMap,
    next_id: AtomicI64,
    next_frame_id: Arc<AtomicU64>,
    shutdown: CancellationToken,
}

impl Drop for UnixTransport {
    fn drop(&mut self) {
        // Header reads intentionally have no idle deadline. Cancel the reader;
        // synchronously dropping the writer half makes peer shutdown observable
        // even when the current-thread runtime cannot schedule another task.
        self.shutdown.cancel();
        self.writer.get_mut().take();
    }
}

impl UnixTransport {
    pub fn from_stream(stream: UnixStream) -> Self {
        let (reader, writer) = stream.into_split();
        let (incoming_tx, incoming_rx) = mpsc::channel(INCOMING_QUEUE);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let frame_id = Arc::new(AtomicU64::new(1));
        let shutdown = CancellationToken::new();

        tokio::spawn(read_loop(
            reader,
            incoming_tx,
            pending.clone(),
            shutdown.clone(),
        ));

        Self {
            writer: Mutex::new(Some(writer)),
            incoming: Mutex::new(incoming_rx),
            pending,
            next_id: AtomicI64::new(1),
            next_frame_id: frame_id,
            shutdown,
        }
    }

    async fn send_payload(&self, payload: Value) -> Result<(), AcpError> {
        let frame = WireFrame::request(self.next_frame_id.fetch_add(1, Ordering::Relaxed), payload);
        let encoded = FrameCodec::encode(&frame)
            .map_err(|error| AcpError::new(-32092, format!("ACP protocol failure: {error}")))?;
        let mut writer = self.writer.lock().await;
        let writer = writer.as_mut().ok_or_else(|| {
            AcpError::new(-32091, "ACP transport failure: local transport closed")
        })?;
        match timeout(WRITE_TIMEOUT, writer.write_all(&encoded)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(AcpError::new(
                -32091,
                format!(
                    "ACP transport failure: local socket write ({:?})",
                    error.kind()
                ),
            )),
            Err(_) => Err(AcpError::new(
                -32091,
                "ACP transport failure: local socket write timed out",
            )),
        }
    }
}

async fn read_frame(reader: &mut OwnedReadHalf) -> Result<WireFrame, FrameError> {
    let mut header = [0_u8; HEADER_BYTES];
    // An idle ACP connection is healthy. Only a partially received frame has a
    // deadline; waiting for the first header byte must not manufacture EOF.
    reader
        .read_exact(&mut header)
        .await
        .map_err(map_read_error)?;
    let payload_len = u32::from_be_bytes(header[14..18].try_into().expect("fixed header")) as usize;
    if payload_len > MAX_FRAME_BYTES {
        return Err(FrameError::FrameTooLarge);
    }
    let mut full = Vec::with_capacity(HEADER_BYTES + payload_len);
    full.extend_from_slice(&header);
    full.resize(HEADER_BYTES + payload_len, 0);
    timeout(
        FRAME_BODY_TIMEOUT,
        reader.read_exact(&mut full[HEADER_BYTES..]),
    )
    .await
    .map_err(|_| FrameError::ReadTimeout)?
    .map_err(|_| FrameError::IncompleteFrame)?;
    FrameCodec::decode(&full)
}

fn map_read_error(error: std::io::Error) -> FrameError {
    match error.kind() {
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::BrokenPipe => {
            FrameError::PeerClosed
        }
        _ => FrameError::IoFailure,
    }
}

fn structured_transport_failure(error: &FrameError) -> IncomingMessage {
    let (classification, reason_code) = match error {
        FrameError::PeerClosed => ("SOCKET_EOF", "ACP_PEER_CLOSED"),
        FrameError::ReadTimeout => ("PROTOCOL_ERROR", "ACP_PARTIAL_FRAME_TIMEOUT"),
        FrameError::IoFailure => ("SOCKET_EOF", "ACP_SOCKET_IO_FAILURE"),
        FrameError::FrameTooLarge => ("PROTOCOL_ERROR", "ACP_FRAME_TOO_LARGE"),
        FrameError::IncompleteFrame => ("PROTOCOL_ERROR", "ACP_INCOMPLETE_FRAME"),
        FrameError::InvalidMagic => ("PROTOCOL_ERROR", "ACP_INVALID_MAGIC"),
        FrameError::IncompatibleProtocol => ("PROTOCOL_ERROR", "ACP_PROTOCOL_VERSION"),
        FrameError::InvalidPayload => ("PROTOCOL_ERROR", "ACP_INVALID_PAYLOAD"),
    };
    IncomingMessage::Notification {
        method: TRANSPORT_CLOSED_METHOD.to_string(),
        params: json!({
            "classification": classification,
            "reason_code": reason_code,
            "message": error.to_string(),
        }),
    }
}

async fn read_loop(
    mut reader: OwnedReadHalf,
    incoming: mpsc::Sender<IncomingMessage>,
    pending: PendingMap,
    shutdown: CancellationToken,
) {
    loop {
        let frame = tokio::select! {
            _ = shutdown.cancelled() => break,
            frame = read_frame(&mut reader) => match frame {
                Ok(frame) => frame,
                Err(error) => {
                    tracing::debug!(error = %error, "local ACP read loop closed");
                    let _ = incoming.send(structured_transport_failure(&error)).await;
                    let mut pending = pending.lock().await;
                    for (_, sender) in pending.drain() {
                        let _ = sender.send(Err(AcpError::new(
                            -32091,
                            format!("ACP transport failure: {error}"),
                        )));
                    }
                    break;
                },
            },
        };
        let Some(message) = decode_acp_message(frame.payload) else {
            tracing::debug!("local ACP payload rejected");
            let error = FrameError::InvalidPayload;
            let _ = incoming.send(structured_transport_failure(&error)).await;
            break;
        };
        if let IncomingMessage::Response {
            id: RequestId::Number(id),
            result,
        } = message
        {
            if let Some(sender) = pending.lock().await.remove(&id) {
                let _ = sender.send(result);
                continue;
            }
            let _ = incoming
                .send(IncomingMessage::Response {
                    id: RequestId::Number(id),
                    result,
                })
                .await;
            continue;
        }
        if incoming.send(message).await.is_err() {
            break;
        }
    }
}

fn decode_acp_message(payload: Value) -> Option<IncomingMessage> {
    let id = payload.get("id").cloned();
    if let Some(method) = payload.get("method").and_then(Value::as_str) {
        let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
        return match id {
            Some(id) => serde_json::from_value(id)
                .ok()
                .map(|id| IncomingMessage::Request {
                    id,
                    method: method.to_string(),
                    params,
                    caller: None,
                }),
            None => Some(IncomingMessage::Notification {
                method: method.to_string(),
                params,
            }),
        };
    }
    let id = id.and_then(|id| serde_json::from_value(id).ok())?;
    if let Some(error) = payload.get("error") {
        let error = serde_json::from_value(error.clone()).ok()?;
        return Some(IncomingMessage::Response {
            id,
            result: Err(error),
        });
    }
    Some(IncomingMessage::Response {
        id,
        result: Ok(payload.get("result").cloned().unwrap_or(Value::Null)),
    })
}

#[async_trait]
impl AcpTransport for UnixTransport {
    async fn send_request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        if let Err(error) = self
            .send_payload(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        receiver
            .await
            .map_err(|_| AcpError::new(-32603, "Local transport closed"))?
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<(), AcpError> {
        self.send_payload(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    async fn recv(&self) -> Option<IncomingMessage> {
        self.incoming.lock().await.recv().await
    }

    async fn send_response(
        &self,
        id: RequestId,
        result: Result<Value, AcpError>,
    ) -> Result<(), AcpError> {
        let payload = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": sanitize_error(error)}),
        };
        self.send_payload(payload).await
    }

    async fn close(&self) -> Result<(), AcpError> {
        self.shutdown.cancel();
        self.writer.lock().await.take();
        Ok(())
    }
}

fn sanitize_error(error: AcpError) -> AcpError {
    AcpError::new(
        error.code,
        error.message.chars().take(240).collect::<String>(),
    )
}
