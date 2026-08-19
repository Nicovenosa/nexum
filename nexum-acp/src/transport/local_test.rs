use std::path::Path;
use std::time::Duration;

use interprocess::local_socket::{
    tokio::{prelude::*, Listener as LocalSocketListener},
    ListenerOptions,
};
use serde_json::json;

use super::{local::LocalAcpTransport, socket::SocketTransport, AcpTransport};

async fn bind_server(socket: &Path) -> LocalSocketListener {
    let name = super::local::local_socket_name(socket).unwrap();
    ListenerOptions::new().name(name).create_tokio().unwrap()
}

#[tokio::test]
async fn test_local_transport_connects_and_waits_for_ready_health() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await;
    let server = tokio::spawn(async move {
        let stream = listener.accept().await.unwrap();
        let transport = SocketTransport::from_stream(stream);
        if let Some(super::types::IncomingMessage::Request { id, method, .. }) =
            transport.recv().await
        {
            assert_eq!(method, "health");
            transport
                .send_response(
                    id,
                    Ok(json!({
                        "protocol_version": super::socket::LOCAL_PROTOCOL_VERSION,
                        "runtime_available": true,
                        "health": "ready"
                    })),
                )
                .await
                .unwrap();
        } else {
            panic!("expected local health request");
        }
    });

    let transport = LocalAcpTransport::connect_ready(&socket, Duration::from_secs(1))
        .await
        .unwrap();

    assert_eq!(transport.socket_path(), socket.as_path());
    server.await.unwrap();
}

#[tokio::test]
async fn test_local_transport_rejects_incompatible_or_unready_health() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await;
    let server = tokio::spawn(async move {
        let stream = listener.accept().await.unwrap();
        let transport = SocketTransport::from_stream(stream);
        if let Some(super::types::IncomingMessage::Request { id, .. }) = transport.recv().await {
            transport
                .send_response(
                    id,
                    Ok(json!({
                        "protocol_version": 999,
                        "runtime_available": true,
                        "health": "starting"
                    })),
                )
                .await
                .unwrap();
        }
    });

    let error = LocalAcpTransport::connect_ready(&socket, Duration::from_secs(1))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("protocol"));
    server.await.unwrap();
}

#[tokio::test]
async fn test_local_transport_preserves_runtime_identity_response() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await;
    let server = tokio::spawn(async move {
        let stream = listener.accept().await.unwrap();
        let transport = SocketTransport::from_stream(stream);
        let Some(super::types::IncomingMessage::Request { id, method, .. }) =
            transport.recv().await
        else {
            panic!("expected health request");
        };
        assert_eq!(method, "health");
        transport
            .send_response(
                id,
                Ok(json!({
                    "protocol_version": super::socket::LOCAL_PROTOCOL_VERSION,
                    "runtime_available": true,
                    "health": "ready"
                })),
            )
            .await
            .unwrap();
        let Some(super::types::IncomingMessage::Request { id, method, .. }) =
            transport.recv().await
        else {
            panic!("expected identity request");
        };
        assert_eq!(method, "runtime/identity");
        transport
            .send_response(
                id,
                Ok(json!({
                    "runtime_instance_id": "host-instance-1",
                    "transport_kind": "unix"
                })),
            )
            .await
            .unwrap();
    });

    let transport = LocalAcpTransport::connect_ready(&socket, Duration::from_secs(1))
        .await
        .unwrap();
    let identity = transport
        .send_request("runtime/identity", json!({}))
        .await
        .unwrap();

    assert_eq!(identity["runtime_instance_id"], "host-instance-1");
    assert_eq!(identity["transport_kind"], "unix");
    server.await.unwrap();
}

#[tokio::test]
async fn test_local_transport_close_ends_the_client_stream_without_touching_host() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await;
    let server = tokio::spawn(async move {
        let stream = listener.accept().await.unwrap();
        let transport = SocketTransport::from_stream(stream);
        let Some(super::types::IncomingMessage::Request { id, .. }) = transport.recv().await else {
            panic!("expected health request");
        };
        transport
            .send_response(
                id,
                Ok(json!({
                    "protocol_version": super::socket::LOCAL_PROTOCOL_VERSION,
                    "runtime_available": true,
                    "health": "ready"
                })),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let Some(super::types::IncomingMessage::Notification { method, params }) =
            transport.recv().await
        else {
            panic!("peer close must be reported before the stream ends");
        };
        assert_eq!(method, super::socket::TRANSPORT_CLOSED_METHOD);
        assert_eq!(params["classification"], "SOCKET_EOF");
        assert_eq!(params["reason_code"], "ACP_PEER_CLOSED");
        assert!(transport.recv().await.is_none());
    });

    let transport = LocalAcpTransport::connect_ready(&socket, Duration::from_secs(1))
        .await
        .unwrap();
    transport.close().await.unwrap();
    assert!(transport.recv().await.is_none());
    server.await.unwrap();
}
