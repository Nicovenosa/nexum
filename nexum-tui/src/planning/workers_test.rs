//! Tests de Workers: 11 escenarios del contrato + 8 gates de la Fase A.

use super::*;
use crate::planning::cartero::StepContext;
use std::collections::BTreeMap;

fn ctx(scope: Vec<&str>, payload: &str, size_override: Option<usize>) -> StepContext {
    let payload = payload.to_string();
    let size = size_override.unwrap_or(payload.len());
    StepContext {
        step_id: "s1".into(),
        capability: "read".into(),
        scope: scope.into_iter().map(String::from).collect(),
        provenance: "prov#s1".into(),
        size_bytes: size,
        secrets_redacted: false,
        payload,
        excluded_fields: vec![],
    }
}

fn ok_handler() -> WorkerHandler {
    std::sync::Arc::new(|_req: &WorkerRequest| {
        let mut o = BTreeMap::new();
        o.insert("status".to_string(), "done".to_string());
        Ok(o)
    })
}

fn valid_contract(id: &str) -> WorkerContract {
    WorkerContract {
        worker_id: id.into(),
        capabilities: vec!["read".into()],
        allowed_scopes: vec!["fs:read".into()],
        timeout: std::time::Duration::from_millis(500),
        requires_approval: false,
        output_keys: vec!["status".into()],
        handler: ok_handler(),
    }
}

fn cancel_off() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
}

// ── 1. worker válido ──
#[test]
fn test_1_worker_valido() {
    let mut reg = WorkerRegistry::new();
    reg.register(valid_contract("w"), true);
    let out = reg
        .dispatch("w", "read", "req-1", ctx(vec!["fs:read"], "leer", None), cancel_off(), true)
        .unwrap();
    assert!(out.ok);
    assert_eq!(out.output.get("status").unwrap(), "done");
}

// ── 2. worker desconocido ──
#[test]
fn test_2_worker_desconocido() {
    let reg = WorkerRegistry::new();
    let e = reg.dispatch("nope", "read", "r", ctx(vec!["fs:read"], "x", None), cancel_off(), true);
    assert_eq!(e.unwrap_err(), WorkerError::Unknown);
}

// ── 3. capability no autorizada (dos vías: no declarada, o Rust no autoriza) ──
#[test]
fn test_3_capability_no_autorizada() {
    let mut reg = WorkerRegistry::new();
    reg.register(valid_contract("w"), true);
    // capability no declarada en el contrato
    let e1 = reg.dispatch("w", "bash", "r1", ctx(vec!["fs:read"], "x", None), cancel_off(), true);
    assert_eq!(e1.unwrap_err(), WorkerError::UnauthorizedCapability);
    // capability declarada pero Rust NO autoriza (el worker no se auto-concede)
    let e2 = reg.dispatch("w", "read", "r2", ctx(vec!["fs:read"], "x", None), cancel_off(), false);
    assert_eq!(e2.unwrap_err(), WorkerError::UnauthorizedCapability);
}

// ── 4. timeout ──
#[test]
fn test_4_timeout() {
    let mut reg = WorkerRegistry::new();
    let slow: WorkerHandler = std::sync::Arc::new(|_r| {
        std::thread::sleep(std::time::Duration::from_millis(400));
        let mut o = BTreeMap::new();
        o.insert("status".to_string(), "done".to_string());
        Ok(o)
    });
    reg.register(
        WorkerContract {
            worker_id: "slow".into(),
            capabilities: vec!["read".into()],
            allowed_scopes: vec!["fs:read".into()],
            timeout: std::time::Duration::from_millis(50),
            requires_approval: false,
            output_keys: vec!["status".into()],
            handler: slow,
        },
        true,
    );
    let e = reg.dispatch("slow", "read", "r", ctx(vec!["fs:read"], "x", None), cancel_off(), true);
    assert_eq!(e.unwrap_err(), WorkerError::Timeout);
}

// ── 5. cancelación ──
#[test]
fn test_5_cancelacion() {
    let mut reg = WorkerRegistry::new();
    reg.register(valid_contract("w"), true);
    let cancel = cancel_off();
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let e = reg.dispatch("w", "read", "r", ctx(vec!["fs:read"], "x", None), cancel, true);
    assert_eq!(e.unwrap_err(), WorkerError::Cancelled);
}

// ── 6. resultado malformado (falta output_key requerida) ──
#[test]
fn test_6_resultado_malformado() {
    let mut reg = WorkerRegistry::new();
    let bad: WorkerHandler = std::sync::Arc::new(|_r| Ok(BTreeMap::new())); // sin "status"
    reg.register(
        WorkerContract {
            worker_id: "bad".into(),
            capabilities: vec!["read".into()],
            allowed_scopes: vec!["fs:read".into()],
            timeout: std::time::Duration::from_millis(500),
            requires_approval: false,
            output_keys: vec!["status".into()],
            handler: bad,
        },
        true,
    );
    let e = reg.dispatch("bad", "read", "r", ctx(vec!["fs:read"], "x", None), cancel_off(), true);
    assert_eq!(e.unwrap_err(), WorkerError::MalformedResult);
}

// ── 7. contexto demasiado grande ──
#[test]
fn test_7_contexto_demasiado_grande() {
    let mut reg = WorkerRegistry::new();
    reg.register(valid_contract("w"), true);
    let big = ctx(vec!["fs:read"], "x", Some(super::super::cartero::MAX_CONTEXT_BYTES + 1));
    let e = reg.dispatch("w", "read", "r", big, cancel_off(), true);
    assert_eq!(e.unwrap_err(), WorkerError::ContextTooLarge);
}

// ── 8. scope leak ──
#[test]
fn test_8_scope_leak() {
    let mut reg = WorkerRegistry::new();
    reg.register(valid_contract("w"), true); // allowed: fs:read
    let leaky = ctx(vec!["fs:read", "proc:exec"], "x", None); // pide más scope del permitido
    let e = reg.dispatch("w", "read", "r", leaky, cancel_off(), true);
    assert_eq!(e.unwrap_err(), WorkerError::ScopeLeak);
}

// ── 9. secret leak ──
#[test]
fn test_9_secret_leak() {
    let mut reg = WorkerRegistry::new();
    reg.register(valid_contract("w"), true);
    // payload con secreto SIN redactar (simula un intento de fuga al worker)
    let leaky = ctx(vec!["fs:read"], "api_key=sk-ABCDEF1234567890abcdefGHIJ", None);
    let e = reg.dispatch("w", "read", "r", leaky, cancel_off(), true);
    assert_eq!(e.unwrap_err(), WorkerError::SecretLeak);
}

// ── 10. doble despacho ──
#[test]
fn test_10_doble_despacho() {
    let mut reg = WorkerRegistry::new();
    reg.register(valid_contract("w"), true);
    let r1 = reg.dispatch("w", "read", "same-id", ctx(vec!["fs:read"], "x", None), cancel_off(), true);
    assert!(r1.is_ok());
    let r2 = reg.dispatch("w", "read", "same-id", ctx(vec!["fs:read"], "x", None), cancel_off(), true);
    assert_eq!(r2.unwrap_err(), WorkerError::DoubleDispatch, "mismo request_id ⇒ no re-ejecuta");
}

// ── 11. worker registrado pero nunca utilizado ──
#[test]
fn test_11_registrado_nunca_despachado() {
    let mut reg = WorkerRegistry::new();
    reg.register(valid_contract("usado"), true);
    reg.register(valid_contract("ocioso"), true);
    reg.dispatch("usado", "read", "r", ctx(vec!["fs:read"], "x", None), cancel_off(), true).unwrap();
    let never = reg.active_never_dispatched();
    assert_eq!(never, vec!["ocioso".to_string()], "detecta el worker activo nunca despachado");
}

// ── Gates agregados de la Fase A ──
#[test]
fn test_gates_fase_a() {
    let mut reg = WorkerRegistry::new();
    reg.register(valid_contract("w"), true);
    // double_execution / double_routing: el guard de doble dispatch garantiza 1 sola ejecución
    reg.dispatch("w", "read", "req", ctx(vec!["fs:read"], "x", None), cancel_off(), true).unwrap();
    let dup = reg.dispatch("w", "read", "req", ctx(vec!["fs:read"], "x", None), cancel_off(), true);
    assert!(dup.is_err(), "double_execution = 0");
    // registered_active_workers_never_dispatched = 0 tras despachar todos
    assert!(reg.active_never_dispatched().is_empty(), "todos los activos despachados");
}
