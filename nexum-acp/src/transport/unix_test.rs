use serde_json::json;
use tokio::net::UnixStream;

use super::unix::{FrameCodec, WireFrame, LOCAL_PROTOCOL_VERSION};
use super::AcpTransport;

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
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let server = super::unix::UnixTransport::from_stream(server_stream);
    let client = super::unix::UnixTransport::from_stream(client_stream);

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
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let server = super::unix::UnixTransport::from_stream(server_stream);
    let client = super::unix::UnixTransport::from_stream(client_stream);

    // The source-level invariant is that header reads have no deadline. The
    // installed-artifact E2E additionally holds the connection past 15 seconds.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

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
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let client = super::unix::UnixTransport::from_stream(client_stream);
    drop(server_stream);

    let Some(super::types::IncomingMessage::Notification { method, params }) = client.recv().await
    else {
        panic!("EOF must be reported before the receive channel closes");
    };
    assert_eq!(method, super::unix::TRANSPORT_CLOSED_METHOD);
    assert_eq!(params["classification"], "SOCKET_EOF");
    assert_eq!(params["reason_code"], "ACP_PEER_CLOSED");
    assert!(client.recv().await.is_none());
}
