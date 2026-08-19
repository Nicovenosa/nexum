use std::{io, sync::Arc};
async fn bind_server(socket: &std::path::Path) -> io::Result<LocalSocketListener> {
    let name = nexum_acp::transport::local::local_socket_name(socket)?;
    interprocess::local_socket::ListenerOptions::new().name(name).create_tokio()
}


use nexum_acp::{
    task::{
        EvidencePolicy, ExecutionBudgetV1, OutputFormat, TaskEnvelopeV1, TaskEnvelopeVersion,
        TaskPriority, TaskRisk, TaskSource,
    },
    transport::{
        local::LocalAcpTransport, mpsc::mpsc_transport_pair, types::IncomingMessage,
        socket::SocketTransport, AcpTransport,
    },
};
use serde_json::json;
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::tokio::Stream as LocalSocketStream;

use super::acp_turn::{
    VoiceAcpClient, VoiceHudBridge, VoiceResultFormatter, VoiceRouteDecision, VoiceSessionState,
    VoiceSessionStore, VoiceTurnController, VoiceTurnState,
};
use crate::acp_client::AcpClientTransport;

fn voice_envelope() -> TaskEnvelopeV1 {
    TaskEnvelopeV1 {
        version: TaskEnvelopeVersion::V1,
        envelope_id: "voice-envelope-1".into(),
        source: TaskSource::Voice,
        objective: "Analizar la arquitectura".into(),
        user_input: "Analizá la arquitectura".into(),
        session_id: String::new(),
        thread_id: String::new(),
        workspace: Some("/workspace".into()),
        constraints: vec!["respuesta breve".into()],
        allowed_tools: vec![],
        evidence_refs: vec![],
        success_criteria: vec!["explicar el resultado".into()],
        output_format: OutputFormat::Text,
        execution_budget: ExecutionBudgetV1 {
            wall_time_ms: Some(100),
            ..Default::default()
        },
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
async fn test_escalate_envia_envelope_voice_sin_historial_y_actualiza_hud() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let client = VoiceAcpClient::from_transport(transport);
    let server_task = tokio::spawn(async move {
        for _ in 0..4 {
            let Some(IncomingMessage::Request {
                id,
                method,
                params: _,
                ..
            }) = server.recv().await
            else {
                panic!("expected ACP request");
            };
            let response = match method.as_str() {
                "health" => json!({"health": "ready"}),
                "runtime/identity" => json!({
                    "runtime_instance_id": "shared-runtime",
                    "provider": "mock-provider",
                    "model": "mock-model"
                }),
                "runtime/capabilities" => json!({"capabilities": {"hash": "shared"}}),
                "session/new" => json!({"sessionId": "voice-session-a", "threadId": "thread-a"}),
                _ => panic!("unexpected request: {method}"),
            };
            server.send_response(id, Ok(response)).await.unwrap();
        }
        let Some(IncomingMessage::Request {
            id, method, params, ..
        }) = server.recv().await
        else {
            panic!("expected voice prompt");
        };
        assert_eq!(method, "session/prompt");
        assert_eq!(params["taskEnvelope"]["source"], "voice");
        assert!(params["taskEnvelope"].get("history").is_none());
        server
            .send_notification(
                "session/update",
                json!({"sessionId": "voice-session-a", "provider": "mock-provider", "model": "mock-model", "text": "respuesta parcial"}),
            )
            .await
            .unwrap();
        server
            .send_notification(
                "peri/agent_event_done",
                json!({"sessionId": "voice-session-a"}),
            )
            .await
            .unwrap();
        server
            .send_response(id, Ok(json!({"resultId": "result-a"})))
            .await
            .unwrap();
    });

    let store = VoiceSessionStore::in_memory();
    let hud = VoiceHudBridge::default();
    let mut controller = VoiceTurnController::new(client, store.clone(), hud);
    let result = controller
        .execute(
            VoiceRouteDecision::Escalate {
                envelope: voice_envelope(),
                reason: "compleja".into(),
            },
            "/workspace",
        )
        .await
        .unwrap();

    assert_eq!(result.trace.runtime_instance_id, "shared-runtime");
    assert_eq!(result.trace.provider.as_deref(), Some("mock-provider"));
    assert_eq!(result.trace.model.as_deref(), Some("mock-model"));
    assert!(result.speakable.contains("respuesta parcial"));
    assert_eq!(result.trace.thread_id, "thread-a");
    assert_eq!(result.full_ref.as_deref(), Some("result-a"));
    assert_eq!(
        store.get().unwrap().last_result_id.as_deref(),
        Some("result-a")
    );
    assert_eq!(result.status, "completed");
    assert_eq!(result.trace.envelope_id, "voice-envelope-1");
    assert_eq!(result.trace.evidence_ref_count, 0);
    assert!(!result.trace.turn_id.is_empty());
    assert_eq!(controller.hud().state(), VoiceTurnState::Completed);
    assert_eq!(
        controller.hud().model().provider.as_deref(),
        Some("mock-provider")
    );
    assert_eq!(
        controller.hud().model().model.as_deref(),
        Some("mock-model")
    );
    server_task.await.unwrap();
}

#[tokio::test]
async fn test_sessions_separadas_y_reconexion_recarga_la_metadata_sin_historial() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let client = VoiceAcpClient::from_transport(transport);
    let server_task = tokio::spawn(async move {
        let mut next = 0;
        while let Some(IncomingMessage::Request {
            id, method, params, ..
        }) = server.recv().await
        {
            let response = match method.as_str() {
                "health" => json!({"health": "ready"}),
                "runtime/identity" => json!({"runtime_instance_id": "runtime-a"}),
                "runtime/capabilities" => json!({"capabilities": {}}),
                "session/new" => {
                    next += 1;
                    json!({"sessionId": format!("voice-{next}"), "threadId": format!("thread-{next}")})
                }
                "session/load" => {
                    assert!(params.get("history").is_none());
                    json!({})
                }
                "session/prompt" => {
                    server
                        .send_notification(
                            "peri/agent_event_done",
                            json!({"sessionId": params["sessionId"]}),
                        )
                        .await
                        .unwrap();
                    json!({})
                }
                _ => panic!("unexpected request {method}"),
            };
            server.send_response(id, Ok(response)).await.unwrap();
        }
    });

    let first = VoiceSessionStore::in_memory();
    let second = VoiceSessionStore::in_memory();
    let first_id = client.ensure_session(&first, "/workspace").await.unwrap();
    let second_id = client.ensure_session(&second, "/workspace").await.unwrap();
    assert_ne!(first_id, second_id);
    client.ensure_session(&first, "/workspace").await.unwrap();

    drop(client);
    server_task.abort();
}

#[tokio::test]
async fn test_budget_cancelado_se_propaga_al_hud_y_cancel_usa_acp() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let client = VoiceAcpClient::from_transport(transport);
    let server_task = tokio::spawn(async move {
        for _ in 0..4 {
            let Some(IncomingMessage::Request { id, method, .. }) = server.recv().await else {
                panic!("expected setup request");
            };
            let response = match method.as_str() {
                "health" => json!({"health": "ready"}),
                "runtime/identity" => json!({"runtime_instance_id": "runtime-budget"}),
                "runtime/capabilities" => json!({"capabilities": {}}),
                "session/new" => json!({"sessionId": "voice-budget", "threadId": "thread-budget"}),
                _ => panic!("unexpected setup request {method}"),
            };
            server.send_response(id, Ok(response)).await.unwrap();
        }
        let Some(IncomingMessage::Request { id, method, .. }) = server.recv().await else {
            panic!("expected prompt");
        };
        assert_eq!(method, "session/prompt");
        server
            .send_notification(
                "session/update",
                json!({"sessionId": "voice-budget", "budget": {"state": "cancelled"}}),
            )
            .await
            .unwrap();
        server.send_response(id, Ok(json!({}))).await.unwrap();
        let Some(IncomingMessage::Notification { method, params }) = server.recv().await else {
            panic!("expected cancel notification");
        };
        assert_eq!(method, "session/cancel");
        assert_eq!(params["sessionId"], "voice-budget");
    });

    let store = VoiceSessionStore::in_memory();
    let mut controller = VoiceTurnController::new(client, store, VoiceHudBridge::default());
    let error = match controller
        .execute(
            VoiceRouteDecision::Escalate {
                envelope: voice_envelope(),
                reason: "compleja".into(),
            },
            "/workspace",
        )
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("el budget cancelado debe interrumpir el turno"),
    };
    assert!(matches!(error, super::acp_turn::VoiceTurnError::Cancelled));
    assert_eq!(controller.hud().state(), VoiceTurnState::Cancelled);
    controller.cancel().await.unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn test_timeout_usa_wall_time_del_envelope_no_un_timeout_fijo() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let client = VoiceAcpClient::from_transport(transport);
    let server_task = tokio::spawn(async move {
        for _ in 0..4 {
            let Some(IncomingMessage::Request { id, method, .. }) = server.recv().await else {
                panic!("expected setup request");
            };
            let response = match method.as_str() {
                "health" => json!({"health": "ready"}),
                "runtime/identity" => json!({"runtime_instance_id": "runtime-timeout"}),
                "runtime/capabilities" => json!({"capabilities": {}}),
                "session/new" => {
                    json!({"sessionId": "voice-timeout", "threadId": "thread-timeout"})
                }
                _ => panic!("unexpected request {method}"),
            };
            server.send_response(id, Ok(response)).await.unwrap();
        }
        let Some(IncomingMessage::Request { id, method, .. }) = server.recv().await else {
            panic!("expected prompt");
        };
        assert_eq!(method, "session/prompt");
        server.send_response(id, Ok(json!({}))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    });
    let mut envelope = voice_envelope();
    envelope.execution_budget.wall_time_ms = Some(1);
    let mut controller = VoiceTurnController::new(
        client,
        VoiceSessionStore::in_memory(),
        VoiceHudBridge::default(),
    );
    let started = std::time::Instant::now();
    let error = match controller
        .execute(
            VoiceRouteDecision::Escalate {
                envelope,
                reason: "compleja".into(),
            },
            "/workspace",
        )
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("el budget de 1 ms debe vencer el turno"),
    };
    assert!(matches!(error, super::acp_turn::VoiceTurnError::TimedOut));
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    server_task.await.unwrap();
}

#[tokio::test]
async fn test_timeout_del_envelope_incluye_la_solicitud_prompt_sin_respuesta() {
    let (transport, server) = mpsc_transport_pair();
    let transport: Arc<dyn AcpClientTransport> = Arc::new(transport);
    let client = VoiceAcpClient::from_transport(transport);
    let server_task = tokio::spawn(async move {
        for _ in 0..4 {
            let Some(IncomingMessage::Request { id, method, .. }) = server.recv().await else {
                panic!("expected setup request");
            };
            let response = match method.as_str() {
                "health" => json!({"health": "ready"}),
                "runtime/identity" => json!({"runtime_instance_id": "runtime-prompt-timeout"}),
                "runtime/capabilities" => json!({"capabilities": {}}),
                "session/new" => {
                    json!({"sessionId": "voice-prompt-timeout", "threadId": "thread-prompt-timeout"})
                }
                _ => panic!("unexpected request {method}"),
            };
            server.send_response(id, Ok(response)).await.unwrap();
        }
        let Some(IncomingMessage::Request { method, .. }) = server.recv().await else {
            panic!("expected prompt");
        };
        assert_eq!(method, "session/prompt");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    });
    let mut envelope = voice_envelope();
    envelope.execution_budget.wall_time_ms = Some(50);
    let mut controller = VoiceTurnController::new(
        client,
        VoiceSessionStore::in_memory(),
        VoiceHudBridge::default(),
    );
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        controller.execute(
            VoiceRouteDecision::Escalate {
                envelope,
                reason: "compleja".into(),
            },
            "/workspace",
        ),
    )
    .await;
    assert!(matches!(
        result,
        Ok(Err(super::acp_turn::VoiceTurnError::TimedOut))
    ));
    assert!(started.elapsed() < std::time::Duration::from_millis(250));
    assert_eq!(controller.hud().state(), VoiceTurnState::TimedOut);
    server_task.abort();
}

#[test]
fn test_formatter_remueve_json_codigo_stack_y_rutas() {
    let formatted = VoiceResultFormatter::format(
        "```json\n{\"token\":\"secret\"}\n```\npanic at src/main.rs:12\n/home/nico/proyecto\nRespuesta útil.",
    );
    assert_eq!(formatted, "Respuesta útil.");
}

#[test]
fn test_timeout_y_cancelacion_se_muestran_sin_datos_tecnicos() {
    let mut hud = VoiceHudBridge::default();
    hud.timeout();
    assert_eq!(hud.state(), VoiceTurnState::TimedOut);
    assert!(!hud.message().contains('/'));
    hud.cancelled();
    assert_eq!(hud.state(), VoiceTurnState::Cancelled);
}

#[test]
fn test_hud_mapea_todos_los_eventos_minimos() {
    let mut hud = VoiceHudBridge::default();
    for (event, expected) in [
        ("connecting", VoiceTurnState::Connecting),
        ("ready", VoiceTurnState::Ready),
        ("routing", VoiceTurnState::Routing),
        ("escalated", VoiceTurnState::Escalating),
        ("model_selected", VoiceTurnState::ModelSelected),
        ("thinking", VoiceTurnState::Thinking),
        ("tool_started", VoiceTurnState::ToolStarted),
        ("tool_finished", VoiceTurnState::ToolFinished),
        ("streaming", VoiceTurnState::Streaming),
        ("budget_warning", VoiceTurnState::BudgetWarning),
        ("budget_exceeded", VoiceTurnState::BudgetExceeded),
        ("completed", VoiceTurnState::Completed),
        ("cancelled", VoiceTurnState::Cancelled),
        ("failed", VoiceTurnState::Failed),
    ] {
        hud.apply_event(event);
        assert_eq!(hud.state(), expected, "evento {event}");
    }
}

#[test]
fn test_metadata_se_escribe_atomicamente_y_se_recarga_sin_historial() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("voice-session.json");
    let store = VoiceSessionStore::at_path(&path);
    store
        .replace(super::acp_turn::VoiceSessionState {
            schema_version: 1,
            session_id: "voice-session".into(),
            thread_id: "thread-session".into(),
            workspace_id: "/workspace".into(),
            last_runtime_instance_id: "runtime".into(),
            last_result_id: None,
            updated_at: 0,
        })
        .unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(!raw.contains("history"));
    assert_eq!(
        VoiceSessionStore::at_path(path).get().unwrap().session_id,
        "voice-session"
    );
}

#[test]
fn test_route_decision_tiene_solo_las_semanticas_explicitas() {
    let decision = VoiceRouteDecision::Local {
        response: super::VoiceResponse::local("listo".into(), super::HudHint::None),
        reason: "respuesta local confiable".into(),
    };
    assert!(decision.validate().is_ok());
    assert!(matches!(
        VoiceRouteDecision::NeedMoreContext {
            missing_fields: vec!["objetivo".into()]
        },
        VoiceRouteDecision::NeedMoreContext { .. }
    ));
}

#[test]
fn test_session_state_tiene_schema_y_solo_los_campos_permitidos() {
    let state = VoiceSessionState {
        schema_version: 1,
        session_id: "session-a".into(),
        thread_id: "thread-a".into(),
        workspace_id: "workspace-a".into(),
        last_runtime_instance_id: "runtime-a".into(),
        last_result_id: Some("result-a".into()),
        updated_at: 42,
    };
    let value = serde_json::to_value(state).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 7);
    assert_eq!(value["thread_id"], "thread-a");
    assert_ne!(value["session_id"], value["thread_id"]);
}

#[tokio::test]
async fn test_voice_y_tui_local_acp_observan_la_misma_instancia_unix() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await.unwrap();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let stream = listener.accept().await.unwrap();
            connections.spawn(async move {
                let transport = SocketTransport::from_stream(stream);
                while let Some(IncomingMessage::Request { id, method, .. }) = transport.recv().await
                {
                    let value = match method.as_str() {
                        "health" => json!({
                            "protocol_version": nexum_acp::transport::socket::LOCAL_PROTOCOL_VERSION,
                            "runtime_available": true,
                            "health": "ready"
                        }),
                        "runtime/identity" => json!({"runtime_instance_id": "unix-runtime"}),
                        "runtime/capabilities" => json!({"capabilities": {"voice": true}}),
                        _ => panic!("unexpected Unix ACP request {method}"),
                    };
                    transport.send_response(id, Ok(value)).await.unwrap();
                }
            });
        }
        while let Some(result) = connections.join_next().await {
            result.unwrap();
        }
    });
    let voice_transport =
        LocalAcpTransport::connect_ready(&socket, std::time::Duration::from_secs(1))
            .await
            .unwrap();
    let tui_transport =
        LocalAcpTransport::connect_ready(&socket, std::time::Duration::from_secs(1))
            .await
            .unwrap();
    let voice = VoiceAcpClient::from_transport(Arc::new(voice_transport));
    let tui: Arc<dyn AcpClientTransport> = Arc::new(tui_transport);
    let (tui, _) = crate::acp_client::AcpTuiClient::new(tui);

    assert_eq!(
        voice.identity().await.unwrap()["runtime_instance_id"],
        "unix-runtime"
    );
    assert_eq!(
        tui.request("runtime/identity", json!({})).await.unwrap()["runtime_instance_id"],
        "unix-runtime"
    );
    voice.close().await.unwrap();
    tui.close().await.unwrap();
    drop(voice);
    drop(tui);
    server.await.unwrap();
}
