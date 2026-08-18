use std::sync::Arc;

use nexum_acp::transport::{mpsc::mpsc_transport_pair, types::IncomingMessage, AcpTransport};
use serde_json::json;

use super::{AcpClientTransport, AcpNotification, AcpTuiClient};

fn stable_envelope() -> nexum_acp::task::TaskEnvelopeV1 {
    use nexum_acp::task::*;
    TaskEnvelopeV1 {
        version: TaskEnvelopeVersion::V1,
        envelope_id: "env-1".into(),
        source: TaskSource::Tui,
        objective: "saludar".into(),
        user_input: "Hola".into(),
        session_id: "session-a".into(),
        thread_id: "session-a".into(),
        workspace: Some("/tmp/workspace".into()),
        constraints: vec![],
        allowed_tools: vec![],
        evidence_refs: vec![],
        success_criteria: vec![],
        output_format: OutputFormat::Text,
        execution_budget: ExecutionBudgetV1::default(),
        evidence_policy: EvidencePolicy {
            require_evidence: false,
            minimum_evidence_refs: 0,
            allow_unverified_output: true,
        },
        priority: TaskPriority::Normal,
        risk: TaskRisk::Low,
        sanitized_metadata: Default::default(),
    }
}

#[tokio::test]
async fn test_client_accepts_object_safe_mpsc_transport_and_pumps_notifications() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let (client, mut notifications) = AcpTuiClient::new(transport);
    client.spawn_pump();

    server
        .send_notification(
            "session/update",
            json!({"sessionId": "session-a", "update": {}}),
        )
        .await
        .unwrap();

    let notification =
        tokio::time::timeout(std::time::Duration::from_secs(1), notifications.recv())
            .await
            .unwrap()
            .unwrap();
    assert!(matches!(
        notification,
        AcpNotification::SessionUpdate { session_id, .. } if session_id == "session-a"
    ));
}

#[tokio::test]
async fn test_client_sends_requests_through_object_safe_transport() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let (client, _notifications) = AcpTuiClient::new(transport);
    let server_task = tokio::spawn(async move {
        if let Some(IncomingMessage::Request {
            id, method, params, ..
        }) = server.recv().await
        {
            assert_eq!(method, "session/new");
            assert_eq!(params["cwd"], "/tmp/workspace");
            server
                .send_response(id, Ok(json!({"sessionId": "session-a"})))
                .await
                .unwrap();
        } else {
            panic!("expected session/new request");
        }
    });

    assert_eq!(
        client.new_session("/tmp/workspace", None).await.unwrap(),
        "session-a"
    );
    server_task.await.unwrap();
}

#[tokio::test]
async fn test_update_config_sends_provider_and_model_together_without_session() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let (client, _notifications) = AcpTuiClient::new(transport);
    let mut config = crate::config::NexumConfig::default();
    config.config.active_provider_id = "catalog-provider".to_string();
    config.config.active_alias = "catalog-model".to_string();

    client.update_config(&config).await.unwrap();

    match server.recv().await {
        Some(IncomingMessage::Notification { method, params }) => {
            assert_eq!(method, "session/config_update");
            assert_eq!(params["config"]["config"]["active_provider_id"], "catalog-provider");
            assert_eq!(params["config"]["config"]["active_alias"], "catalog-model");
        }
        other => panic!("expected config update notification, got {other:?}"),
    }
}

#[tokio::test]
async fn test_red_transport_eof_emits_one_terminal_notification() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let (client, mut notifications) = AcpTuiClient::new(transport);
    client.spawn_pump();
    drop(server);

    let notification =
        tokio::time::timeout(std::time::Duration::from_secs(1), notifications.recv())
            .await
            .expect("transport EOF must produce a terminal notification")
            .expect("transport EOF must not close the TUI notification channel silently");

    assert!(matches!(notification, AcpNotification::TurnFailed { .. }));
}

#[tokio::test]
async fn structured_transport_failure_is_forwarded_exactly_once() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let (client, mut notifications) = AcpTuiClient::new(transport);
    client.spawn_pump();
    server
        .send_notification(
            nexum_acp::transport::unix::TRANSPORT_CLOSED_METHOD,
            json!({
                "classification": "SOCKET_EOF",
                "reason_code": "ACP_PEER_CLOSED",
                "message": "local transport peer closed"
            }),
        )
        .await
        .unwrap();
    drop(server);

    let first = notifications.recv().await.unwrap();
    assert!(matches!(
        first,
        AcpNotification::TurnFailed { message }
            if message.contains("SOCKET_EOF/ACP_PEER_CLOSED")
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), notifications.recv())
            .await
            .is_err(),
        "structured close must not be followed by a generic duplicate terminal"
    );
}

#[tokio::test]
async fn acp_bound_route_sends_task_envelope() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let (client, _notifications) = AcpTuiClient::new(transport);
    let server_task = tokio::spawn(async move {
        for expected_method in ["session/new", "session/prompt"] {
            let Some(IncomingMessage::Request {
                id, method, params, ..
            }) = server.recv().await
            else {
                panic!("expected {expected_method}");
            };
            assert_eq!(method, expected_method);
            if method == "session/prompt" {
                assert_eq!(params["stableProfile"], true);
                assert_eq!(params["taskEnvelope"]["envelope_id"], "env-1");
                assert_eq!(params["taskEnvelope"]["source"], "tui");
            }
            let response = if method == "session/new" {
                json!({"sessionId": "session-a"})
            } else {
                json!({})
            };
            server.send_response(id, Ok(response)).await.unwrap();
        }
    });

    client.new_session("/tmp/workspace", None).await.unwrap();
    client
        .prompt_with_task_envelope(
            &nexum_agent::messages::MessageContent::text("Hola"),
            &stable_envelope(),
        )
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn stable_tui_route_never_sends_prompt_without_envelope() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let (client, _notifications) = AcpTuiClient::new(transport);
    assert!(client
        .prompt_with_task_envelope(
            &nexum_agent::messages::MessageContent::text("Hola"),
            &stable_envelope(),
        )
        .await
        .is_err());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), server.recv())
            .await
            .is_err(),
        "without an active session the stable client must send no legacy prompt"
    );
}
