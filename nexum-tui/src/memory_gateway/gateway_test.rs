use super::*;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::ui::demo_mode::test_env_lock()
}

#[test]
fn test_map_error_codigos_del_contrato() {
    assert_eq!(
        map_error(401, r#"{"ok":false,"code":"MG_AUTH_01","message":"unauthorized"}"#),
        MemoryError::Auth
    );
    assert_eq!(
        map_error(422, r#"{"ok":false,"code":"MG_WRITE_01","message":"x"}"#),
        MemoryError::NotConfirmed
    );
    let busy = map_error(503, r#"{"ok":false,"code":"MG_DB_02","message":"x"}"#);
    assert_eq!(busy, MemoryError::DbBusy);
    assert!(busy.retryable(), "MG_DB_02 es el único retryable");
    let quar = map_error(503, r#"{"ok":false,"code":"MG_DB_03","message":"aislada"}"#);
    assert!(matches!(quar, MemoryError::DbQuarantined(_)));
    assert!(!quar.retryable());
    assert!(matches!(
        map_error(400, r#"{"ok":false,"code":"MG_SCOPE_01","message":"scope"}"#),
        MemoryError::InvalidPayload(_)
    ));
    assert!(matches!(
        map_error(404, r#"{"ok":false,"code":"MG_GET_01","message":"no"}"#),
        MemoryError::NotFound(_)
    ));
    assert_eq!(
        map_error(413, r#"{"ok":false,"code":"MG_HTTP_13","message":"grande"}"#),
        MemoryError::TooLarge
    );
    // body no-JSON: fail-closed a Server, jamás panic
    assert!(matches!(map_error(500, "boom"), MemoryError::Server(_)));
}

#[test]
fn test_flag_off_por_defecto_y_override_de_sesion() {
    let _guard = env_lock();
    std::env::remove_var("NEXUM_MEMORY");
    let st = MemoryUiState::default();
    assert!(!env_flag_on(), "flag OFF por defecto (D-13)");
    assert!(!enabled(&st));
    std::env::set_var("NEXUM_MEMORY", "on");
    assert!(enabled(&st));
    let apagada = MemoryUiState {
        session_enabled: Some(false),
        ..Default::default()
    };
    assert!(!enabled(&apagada), "/memoria off gana aunque el flag esté on");
    std::env::remove_var("NEXUM_MEMORY");
}

#[test]
fn test_backend_caido_degradacion_explicita_sin_inventar() {
    let _guard = env_lock();
    // Runtime dir apuntando a la nada: discovery falla ⇒ Unavailable.
    std::env::set_var("NEXUM_MEMORY_RUNTIME_DIR", "/nonexistent-memgw-test");
    let r = client::recall("query", "user", "local");
    assert!(matches!(r, Err(MemoryError::Unavailable(_))));
    let h = client::health();
    assert!(matches!(h, Err(MemoryError::Unavailable(_))));
    std::env::remove_var("NEXUM_MEMORY_RUNTIME_DIR");
}

/// Mock one-shot del gateway con el header del contrato de memoria.
fn mock_gateway(status: u16, body: &str, delay_ms: u64) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let port = listener.local_addr().unwrap().port();
    let body = body.to_string();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            if delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

fn setup_discovery(port: u16) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("memory.port"), port.to_string()).unwrap();
    std::fs::write(dir.path().join("memory.token"), "tok-test").unwrap();
    std::env::set_var("NEXUM_MEMORY_RUNTIME_DIR", dir.path());
    dir
}

#[test]
fn test_save_confirmado_parsea_dto_del_contrato() {
    let _guard = env_lock();
    let port = mock_gateway(
        200,
        r#"{"ok":true,"id":"abc-123","deduplicated":false,"conflict":null}"#,
        0,
    );
    let _dir = setup_discovery(port);
    let p = PendingProposal {
        content: "trabajo en X".into(),
        key: Some("trabajo-en".into()),
        scope_type: "user".into(),
        scope_id: "local".into(),
        source_reference: "test".into(),
        idempotency_key: "idem-1".into(),
    };
    let r = client::save_confirmed(&p).expect("save ok");
    assert_eq!(r.id, "abc-123");
    assert!(r.conflict.is_none());
    std::env::remove_var("NEXUM_MEMORY_RUNTIME_DIR");
}

#[test]
fn test_conflicto_estructurado_llega_al_cliente() {
    let _guard = env_lock();
    let body = r#"{"ok":true,"id":"n2","deduplicated":false,"conflict":{"group_id":"g1","key":"trabajo","scope_type":"user","scope_id":"local","status":"open","created_at":1,"resolved_at":null,"winner_id":null,"resolution_note":null,"entries":[{"id":"n1","key":"trabajo","content":"trabajo en X","scope_type":"user","scope_id":"local","created_at":1,"updated_at":1,"source_type":"user_explicit","source_reference":"t","status":"in_conflict","version":1,"checksum":"c1","contradiction_group":"g1"},{"id":"n2","key":"trabajo","content":"trabajo en Y","scope_type":"user","scope_id":"local","created_at":2,"updated_at":2,"source_type":"user_explicit","source_reference":"t","status":"in_conflict","version":1,"checksum":"c2","contradiction_group":"g1"}]}}"#;
    let port = mock_gateway(200, body, 0);
    let _dir = setup_discovery(port);
    let p = PendingProposal {
        content: "trabajo en Y".into(),
        key: Some("trabajo-en".into()),
        scope_type: "user".into(),
        scope_id: "local".into(),
        source_reference: "test".into(),
        idempotency_key: "idem-2".into(),
    };
    let r = client::save_confirmed(&p).expect("save ok");
    let c = r.conflict.expect("conflicto presente");
    assert_eq!(c.status, "open");
    assert_eq!(c.entries.len(), 2, "ambas versiones conservadas (gate 5)");
    std::env::remove_var("NEXUM_MEMORY_RUNTIME_DIR");
}

#[test]
fn test_timeout_visible_y_recuperable() {
    let _guard = env_lock();
    let port = mock_gateway(200, r#"{"ok":true}"#, 3000);
    let _dir = setup_discovery(port);
    let t0 = std::time::Instant::now();
    let r = client::recall("x", "user", "local");
    assert_eq!(r.unwrap_err(), MemoryError::Timeout);
    assert!(t0.elapsed() < std::time::Duration::from_millis(1500), "corta por presupuesto");
    std::env::remove_var("NEXUM_MEMORY_RUNTIME_DIR");
}

#[test]
fn test_error_del_gateway_mapeado_no_inventa_exito() {
    let _guard = env_lock();
    let port = mock_gateway(422, r#"{"ok":false,"code":"MG_WRITE_01","message":"sin confirmar"}"#, 0);
    let _dir = setup_discovery(port);
    let p = PendingProposal {
        content: "x".into(),
        key: None,
        scope_type: "user".into(),
        scope_id: "local".into(),
        source_reference: "t".into(),
        idempotency_key: "i".into(),
    };
    assert_eq!(client::save_confirmed(&p).unwrap_err(), MemoryError::NotConfirmed);
    std::env::remove_var("NEXUM_MEMORY_RUNTIME_DIR");
}
