//! Operaciones del contrato contra el sidecar. Discovery por archivos
//! (`memory.{port,token}` en el runtime dir), presupuestos duros, retry
//! ÚNICAMENTE para errores retryable (MG_DB_02), fail-closed en todo lo
//! demás. Con backend caído: degradación explícita — jamás inventar
//! recuerdos.

use std::path::PathBuf;
use std::time::Duration;

use super::http::{self, TransportError};
use super::types::*;

/// Presupuesto duro por operación (el gateway responde en ~1-3 ms).
const OP_BUDGET: Duration = Duration::from_millis(800);
/// Presupuesto para status (fuera del hot path).
const STATUS_BUDGET: Duration = Duration::from_millis(1500);
/// Espera antes del único retry por DB ocupada.
const BUSY_RETRY_DELAY: Duration = Duration::from_millis(150);

/// Runtime dir — MISMA prioridad que el sidecar Python:
/// env explícita > $XDG_RUNTIME_DIR/nexum-memory.
fn runtime_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("NEXUM_MEMORY_RUNTIME_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("nexum-memory"));
        }
    }
    None
}

/// Lee (puerto, token) publicados por la instancia viva. Token jamás loggeado.
fn discover() -> Option<(u16, String)> {
    let dir = runtime_dir()?;
    let port: u16 = std::fs::read_to_string(dir.join("memory.port"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let token = std::fs::read_to_string(dir.join("memory.token"))
        .ok()?
        .trim()
        .to_string();
    if token.is_empty() {
        return None;
    }
    Some((port, token))
}

fn post_raw(path: &str, body: &str, budget: Duration) -> Result<String, MemoryError> {
    let Some((port, token)) = discover() else {
        return Err(MemoryError::Unavailable("sidecar no descubrible".into()));
    };
    let attempt = |_: u32| -> Result<String, MemoryError> {
        match http::request(port, "POST", path, Some(&token), Some(body), budget) {
            Ok(resp) if resp.status == 200 => Ok(resp.body),
            Ok(resp) => Err(map_error(resp.status, &resp.body)),
            Err(TransportError::Timeout) => Err(MemoryError::Timeout),
            Err(TransportError::Io(e)) => Err(MemoryError::Unavailable(e)),
        }
    };
    match attempt(0) {
        // Retry ÚNICO y solo para lo explícitamente retryable del contrato.
        Err(e) if e.retryable() => {
            std::thread::sleep(BUSY_RETRY_DELAY);
            attempt(1)
        }
        other => other,
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: String) -> Result<T, MemoryError> {
    serde_json::from_str(&body).map_err(|e| MemoryError::Protocol(e.to_string()))
}

pub fn health() -> Result<HealthResponse, MemoryError> {
    let Some((port, _)) = discover() else {
        return Err(MemoryError::Unavailable("sidecar no descubrible".into()));
    };
    match http::request(port, "GET", "/health", None, None, OP_BUDGET) {
        Ok(resp) if resp.status == 200 => parse(resp.body),
        Ok(resp) => Err(map_error(resp.status, &resp.body)),
        Err(TransportError::Timeout) => Err(MemoryError::Timeout),
        Err(TransportError::Io(e)) => Err(MemoryError::Unavailable(e)),
    }
}

pub fn status() -> Result<StatusResponse, MemoryError> {
    let Some((port, token)) = discover() else {
        return Err(MemoryError::Unavailable("sidecar no descubrible".into()));
    };
    match http::request(port, "GET", "/status", Some(&token), None, STATUS_BUDGET) {
        Ok(resp) if resp.status == 200 => parse(resp.body),
        Ok(resp) => Err(map_error(resp.status, &resp.body)),
        Err(TransportError::Timeout) => Err(MemoryError::Timeout),
        Err(TransportError::Io(e)) => Err(MemoryError::Unavailable(e)),
    }
}

/// Persiste una propuesta CONFIRMADA por el usuario. Este es el único
/// camino de escritura estable del runtime: runtime → validación →
/// confirmación → MemoryGateway. Un modelo jamás escribe directo.
pub fn save_confirmed(p: &PendingProposal) -> Result<SaveResponse, MemoryError> {
    let body = serde_json::json!({
        "confirmed": true,
        "content": p.content,
        "key": p.key,
        "scope_type": p.scope_type,
        "scope_id": p.scope_id,
        "source_type": "user_explicit",
        "source_reference": p.source_reference,
        "idempotency_key": p.idempotency_key,
    })
    .to_string();
    parse(post_raw("/save", &body, OP_BUDGET)?)
}

pub fn recall(query: &str, scope_type: &str, scope_id: &str) -> Result<RecallResponse, MemoryError> {
    let body = serde_json::json!({
        "query": query, "scope_type": scope_type, "scope_id": scope_id, "limit": 10,
    })
    .to_string();
    parse(post_raw("/recall", &body, OP_BUDGET)?)
}

pub fn list(scope_type: &str, scope_id: &str) -> Result<ListResponse, MemoryError> {
    let body = serde_json::json!({
        "scope_type": scope_type, "scope_id": scope_id, "limit": 50,
    })
    .to_string();
    parse(post_raw("/list", &body, OP_BUDGET)?)
}

pub fn get(id: &str, scope_type: &str, scope_id: &str) -> Result<GetResponse, MemoryError> {
    let body = serde_json::json!({
        "id": id, "scope_type": scope_type, "scope_id": scope_id,
    })
    .to_string();
    parse(post_raw("/get", &body, OP_BUDGET)?)
}

pub fn delete(id: &str, scope_type: &str, scope_id: &str) -> Result<DeleteResponse, MemoryError> {
    let body = serde_json::json!({
        "id": id, "scope_type": scope_type, "scope_id": scope_id,
    })
    .to_string();
    parse(post_raw("/delete", &body, OP_BUDGET)?)
}

pub fn open_conflicts(scope_type: &str, scope_id: &str) -> Result<ConflictsResponse, MemoryError> {
    let body = serde_json::json!({
        "scope_type": scope_type, "scope_id": scope_id,
    })
    .to_string();
    parse(post_raw("/contradictions", &body, OP_BUDGET)?)
}

pub fn resolve(
    group_id: &str,
    scope_type: &str,
    scope_id: &str,
    mode: &str,
    winner_id: Option<&str>,
    note: &str,
) -> Result<ResolveResponse, MemoryError> {
    let body = serde_json::json!({
        "group_id": group_id, "scope_type": scope_type, "scope_id": scope_id,
        "resolution_mode": mode, "winner_id": winner_id, "resolution_note": note,
    })
    .to_string();
    parse(post_raw("/resolve", &body, OP_BUDGET)?)
}

/// R-4: crear base nueva tras cuarentena — SOLO por acción explícita.
pub fn reset_after_quarantine() -> Result<serde_json::Value, MemoryError> {
    parse(post_raw("/reset", "{}", STATUS_BUDGET)?)
}
