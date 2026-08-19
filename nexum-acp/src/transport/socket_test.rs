use std::time::Duration;

use interprocess::local_socket::{
    tokio::{prelude::*, Listener as LocalSocketListener, Stream as LocalSocketStream},
    ListenerOptions,
};
use serde_json::json;
use tempfile::TempDir;

use super::socket::{FrameCodec, WireFrame, LOCAL_PROTOCOL_VERSION};
use super::AcpTransport;

fn socket_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("acp.sock")
}

fn local_name(path: &std::path::Path) -> interprocess::local_socket::Name<'_> {
    super::local::local_socket_name(path).unwrap()
}

async fn bind_test_listener(path: &std::path::Path) -> LocalSocketListener {
    ListenerOptions::new().name(local_name(path)).create_tokio().unwrap()
}

async fn connect_test_stream(path: std::path::PathBuf) -> LocalSocketStream {
    LocalSocketStream::connect(local_name(&path)).await.unwrap()
}

#[test]
fn test_frame_codec_round_trip_preserves_acp_payload() {
    let frame = WireFrame::request(
        7,
        json!({
            "id": 42,
            "method": "health",
            "params": {}
        }),
    );
    let encoded = FrameCodec::encode(&frame).unwrap();
    let decoded = FrameCodec::decode(&encoded).unwrap();

    assert_eq!(decoded.protocol_version, LOCAL_PROTOCOL_VERSION);
    assert_eq!(decoded.request_id, 7);
    assert_eq!(decoded.payload, frame.payload);
}

#[test]
fn test_frame_codec_rejects_payload_larger_than_limit() {
    let frame = WireFrame::request(1, json!({"payload": "x".repeat(1024 * 1024)}));

    assert!(FrameCodec::encode(&frame).is_err());
}

#[tokio::test]
async fn test_unix_transport_round_trips_request_response() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    let listener = bind_test_listener(&path).await;
    let client_task = tokio::spawn(connect_test_stream(path.clone()));
    let server_conn = listener.accept().await.unwrap();
    let client_stream = client_task.await.unwrap();
    let server = super::socket::SocketTransport::from_stream(server_conn);
    let client = super::socket::SocketTransport::from_stream(client_stream);

    let task = tokio::spawn(async move {
        if let Some(super::types::IncomingMessage::Request { id, params, .. }) = server.recv().await
        {
            server.send_response(id, Ok(params)).await.unwrap();
        }
    });
    let result = client
        .send_request("health", json!({"ok": true}))
        .await
        .unwrap();

    assert_eq!(result, json!({"ok": true}));
    task.await.unwrap();
}

#[tokio::test]
async fn healthy_idle_connection_survives_without_header_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    let listener = bind_test_listener(&path).await;
    let client_task = tokio::spawn(connect_test_stream(path.clone()));
    let server_conn = listener.accept().await.unwrap();
    let client_stream = client_task.await.unwrap();
    let server = super::socket::SocketTransport::from_stream(server_conn);
    let client = super::socket::SocketTransport::from_stream(client_stream);

    // The source-level invariant is that header reads have no deadline. The
    // installed-artifact E2E additionally holds the connection past 15 seconds.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let task = tokio::spawn(async move {
        let Some(super::types::IncomingMessage::Request { id, method, .. }) = server.recv().await
        else {
            panic!("healthy idle transport closed");
        };
        assert_eq!(method, "health");
        server
            .send_response(id, Ok(json!({"health": "ready"})))
            .await
            .unwrap();
    });
    assert_eq!(
        client.send_request("health", json!({})).await.unwrap(),
        json!({"health": "ready"})
    );
    task.await.unwrap();
}

#[tokio::test]
async fn transport_eof_produces_structured_terminal_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    let listener = bind_test_listener(&path).await;
    let client_task = tokio::spawn(connect_test_stream(path.clone()));
    let server_conn = listener.accept().await.unwrap();
    let client_stream = client_task.await.unwrap();
    let client = super::socket::SocketTransport::from_stream(client_stream);
    drop(server_conn);

    let Some(super::types::IncomingMessage::Notification { method, params }) = client.recv().await
    else {
        panic!("EOF must be reported before the receive channel closes");
    };
    assert_eq!(method, super::socket::TRANSPORT_CLOSED_METHOD);
    assert_eq!(params["classification"], "SOCKET_EOF");
    assert_eq!(params["reason_code"], "ACP_PEER_CLOSED");
    assert!(client.recv().await.is_none());
}

#[tokio::test]
async fn close_propagates_eof_to_peer() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    let listener = bind_test_listener(&path).await;
    let client_task = tokio::spawn(connect_test_stream(path));
    let server_conn = listener.accept().await.unwrap();
    let client_stream = client_task.await.unwrap();
    let server = super::socket::SocketTransport::from_stream(server_conn);
    let client = super::socket::SocketTransport::from_stream(client_stream);

    client.close().await.unwrap();
    let Some(super::types::IncomingMessage::Notification { method, .. }) = server.recv().await
    else {
        panic!("close must propagate EOF as TRANSPORT_CLOSED");
    };
    assert_eq!(method, super::socket::TRANSPORT_CLOSED_METHOD);
}
