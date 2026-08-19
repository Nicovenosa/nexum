use std::sync::Arc;

use interprocess::local_socket::tokio::prelude::*;
use serde_json::json;

use super::{
    hub::AcpHub, mpsc::mpsc_transport_pair, types::HostPrincipal, socket::SocketTransport,
    AcpTransport,
};

#[tokio::test]
async fn test_hub_routes_responses_and_session_events_to_owner() {
    let (client_one, peer_one) = mpsc_transport_pair();
    let (client_two, peer_two) = mpsc_transport_pair();
    let client_one = Arc::new(client_one);
    let client_two = Arc::new(client_two);
    let hub = Arc::new(AcpHub::new(2));
    hub.attach(Arc::new(peer_one)).unwrap();
    hub.attach(Arc::new(peer_two)).unwrap();

    let first_client = Arc::clone(&client_one);
    let first = tokio::spawn(async move {
        first_client
            .send_request("session/new", json!({"cwd": "."}))
            .await
    });
    let incoming = hub.recv().await.unwrap();
    let id = match incoming {
        super::types::IncomingMessage::Request { id, .. } => id,
        _ => panic!("expected request"),
    };
    hub.send_response(id, Ok(json!({"sessionId": "one"})))
        .await
        .unwrap();
    assert_eq!(first.await.unwrap().unwrap(), json!({"sessionId": "one"}));

    hub.send_notification("session/update", json!({"sessionId": "one", "update": {}}))
        .await
        .unwrap();
    let notification = client_one.recv().await.unwrap();
    assert!(matches!(
        notification,
        super::types::IncomingMessage::Notification { .. }
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), client_two.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn test_hub_routes_two_unix_clients_without_second_server_transport() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.sock");
    let listener = super::local::local_socket_name(&path).unwrap();
    let listener =
        interprocess::local_socket::ListenerOptions::new().name(listener).create_tokio().unwrap();
    let client_one_conn = super::local::local_socket_name(&path).unwrap();
    let client_one_stream =
        interprocess::local_socket::tokio::Stream::connect(client_one_conn).await.unwrap();
    let peer_one_stream = listener.accept().await.unwrap();
    let client_two_conn = super::local::local_socket_name(&path).unwrap();
    let client_two_stream =
        interprocess::local_socket::tokio::Stream::connect(client_two_conn).await.unwrap();
    let peer_two_stream = listener.accept().await.unwrap();

    let client_one = Arc::new(SocketTransport::from_stream(client_one_stream));
    let client_two = Arc::new(SocketTransport::from_stream(client_two_stream));
    let hub = Arc::new(AcpHub::new(2));
    hub.attach(Arc::new(SocketTransport::from_stream(peer_one_stream)))
        .unwrap();
    hub.attach(Arc::new(SocketTransport::from_stream(peer_two_stream)))
        .unwrap();

    let request_client = Arc::clone(&client_two);
    let request = tokio::spawn(async move {
        request_client
            .send_request("session/new", json!({"cwd": "."}))
            .await
    });
    let id = match hub.recv().await.unwrap() {
        super::types::IncomingMessage::Request { id, .. } => id,
        _ => panic!("expected Unix request"),
    };
    hub.send_response(id, Ok(json!({"sessionId": "unix-two"})))
        .await
        .unwrap();
    assert_eq!(
        request.await.unwrap().unwrap(),
        json!({"sessionId": "unix-two"})
    );

    hub.send_notification(
        "session/update",
        json!({"sessionId": "unix-two", "update": {}}),
    )
    .await
    .unwrap();
    assert!(matches!(
        client_two.recv().await,
        Some(super::types::IncomingMessage::Notification { .. })
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), client_one.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn test_hub_routes_hitl_and_cancel_to_session_owner() {
    let (client, peer) = mpsc_transport_pair();
    let client = Arc::new(client);
    let hub = Arc::new(AcpHub::new(1));
    hub.attach(Arc::new(peer)).unwrap();

    let new_client = Arc::clone(&client);
    let new_session = tokio::spawn(async move {
        new_client
            .send_request("session/new", json!({"cwd": "."}))
            .await
    });
    let id = match hub.recv().await.unwrap() {
        super::types::IncomingMessage::Request { id, .. } => id,
        _ => panic!("expected session request"),
    };
    hub.send_response(id, Ok(json!({"sessionId": "owned"})))
        .await
        .unwrap();
    new_session.await.unwrap().unwrap();

    client
        .send_notification("session/cancel", json!({"sessionId": "owned"}))
        .await
        .unwrap();
    assert!(
        matches!(hub.recv().await, Some(super::types::IncomingMessage::Notification { method, .. }) if method == "session/cancel")
    );

    let hitl_hub = Arc::clone(&hub);
    let hitl = tokio::spawn(async move {
        hitl_hub
            .send_request("session/request_permission", json!({"sessionId": "owned"}))
            .await
    });
    let request = client.recv().await.unwrap();
    let request_id = match request {
        super::types::IncomingMessage::Request { id, method, .. } => {
            assert_eq!(method, "session/request_permission");
            id
        }
        _ => panic!("expected HITL request"),
    };
    client
        .send_response(
            request_id,
            Ok(json!({"outcome": {"optionId": "reject_once"}})),
        )
        .await
        .unwrap();
    assert!(hitl.await.unwrap().is_ok());
}

#[tokio::test]
async fn test_hub_no_permite_a_cliente_b_apropiarse_de_sesion_de_a() {
    let (client_a, peer_a) = mpsc_transport_pair();
    let (client_b, peer_b) = mpsc_transport_pair();
    let client_a = Arc::new(client_a);
    let client_b = Arc::new(client_b);
    let hub = Arc::new(AcpHub::new(2));
    hub.attach(Arc::new(peer_a)).unwrap();
    hub.attach(Arc::new(peer_b)).unwrap();

    let new_session_client = Arc::clone(&client_a);
    let new_session = tokio::spawn(async move {
        new_session_client
            .send_request("session/new", json!({"cwd": "."}))
            .await
    });
    let (id, caller_a) = match hub.recv().await.unwrap() {
        super::types::IncomingMessage::Request {
            id,
            caller: Some(caller),
            ..
        } => (id, caller),
        _ => panic!("expected session/new request"),
    };
    hub.send_response(id, Ok(json!({"sessionId": "thread-a"})))
        .await
        .unwrap();
    new_session.await.unwrap().unwrap();
    assert!(hub.caller_owns_session(&caller_a, "thread-a").await);

    let hijack_client = Arc::clone(&client_b);
    let hijack = tokio::spawn(async move {
        hijack_client
            .send_request(
                "session/prompt",
                json!({"sessionId": "thread-a", "prompt": []}),
            )
            .await
    });
    let (id, caller_b) = match hub.recv().await.unwrap() {
        super::types::IncomingMessage::Request {
            id,
            caller: Some(caller),
            ..
        } => (id, caller),
        _ => panic!("expected session/prompt request"),
    };
    assert!(
        !hub.caller_owns_session(&caller_b, "thread-a").await,
        "el contexto efímero de B no autoriza la sesión de A"
    );
    hub.send_response(id, Ok(json!({}))).await.unwrap();
    hijack.await.unwrap().unwrap();

    hub.send_notification(
        "session/update",
        json!({"sessionId": "thread-a", "update": {}}),
    )
    .await
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_millis(50), client_a.recv()).await,
        Ok(Some(super::types::IncomingMessage::Notification { .. }))
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), client_b.recv())
            .await
            .is_err(),
        "el cliente B no puede sustituir la ruta de la sesión de A"
    );
}

#[tokio::test]
async fn test_hub_reconecta_subscription_al_cargar_sesion() {
    let (client_a, peer_a) = mpsc_transport_pair();
    let client_a = Arc::new(client_a);
    let hub = Arc::new(AcpHub::new(2));
    let principal = HostPrincipal::new("reconnecting-owner").unwrap();
    hub.attach_with_principal(Arc::new(peer_a), principal.clone())
        .unwrap();

    let new_session_client = Arc::clone(&client_a);
    let new_session = tokio::spawn(async move {
        new_session_client
            .send_request("session/new", json!({"cwd": "."}))
            .await
    });
    let (id, caller) = match hub.recv().await.unwrap() {
        super::types::IncomingMessage::Request {
            id,
            caller: Some(caller),
            ..
        } => (id, caller),
        _ => panic!("expected session/new request"),
    };
    assert_eq!(caller.principal(), Some(&principal));
    hub.send_response(id, Ok(json!({"sessionId": "thread-a"})))
        .await
        .unwrap();
    new_session.await.unwrap().unwrap();
    drop(client_a);

    let (client_reconnected, peer_reconnected) = mpsc_transport_pair();
    let client_reconnected = Arc::new(client_reconnected);
    hub.attach_with_principal(Arc::new(peer_reconnected), principal.clone())
        .unwrap();
    let load_client = Arc::clone(&client_reconnected);
    let load = tokio::spawn(async move {
        load_client
            .send_request("session/load", json!({"sessionId": "thread-a", "cwd": "."}))
            .await
    });
    let (id, caller) = match hub.recv().await.unwrap() {
        super::types::IncomingMessage::Request {
            id,
            caller: Some(caller),
            ..
        } => (id, caller),
        _ => panic!("expected session/load request"),
    };
    assert_eq!(caller.principal(), Some(&principal));
    hub.send_response(id, Ok(json!({}))).await.unwrap();
    load.await.unwrap().unwrap();

    hub.send_notification(
        "session/update",
        json!({"sessionId": "thread-a", "update": {}}),
    )
    .await
    .unwrap();
    assert!(matches!(
        client_reconnected.recv().await,
        Some(super::types::IncomingMessage::Notification { .. })
    ));
}

#[tokio::test]
async fn test_hub_propaga_principal_host_autenticado_sin_persistir_conexion() {
    let (client, peer) = mpsc_transport_pair();
    let client = Arc::new(client);
    let hub = Arc::new(AcpHub::new(1));
    let principal = HostPrincipal::new("mock-owner").unwrap();
    hub.attach_with_principal(Arc::new(peer), principal.clone())
        .unwrap();

    let request_client = Arc::clone(&client);
    let request = tokio::spawn(async move {
        request_client
            .send_request("session/new", json!({"cwd": "."}))
            .await
    });
    let (id, caller) = match hub.recv().await.unwrap() {
        super::types::IncomingMessage::Request {
            id,
            caller: Some(caller),
            ..
        } => (id, caller),
        _ => panic!("expected session/new request with caller context"),
    };
    assert_eq!(caller.principal(), Some(&principal));
    hub.send_response(id, Ok(json!({"sessionId": "thread-a"})))
        .await
        .unwrap();
    request.await.unwrap().unwrap();
}
