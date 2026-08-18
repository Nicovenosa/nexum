//! DTOs del contrato MemoryGateway v0.1 + mapeo de errores versionados.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct EntryDto {
    pub id: String,
    pub key: Option<String>,
    pub content: String,
    pub scope_type: String,
    pub scope_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub source_type: String,
    pub source_reference: String,
    pub status: String,
    pub version: i64,
    pub checksum: String,
    pub contradiction_group: Option<String>,
    /// Score bm25 del store. `None` = el backend no supo rankear (camino LIKE
    /// sin FTS5), que NO es lo mismo que "poco relevante" — ver
    /// [`crate::memory_gateway::inyeccion`].
    #[serde(default)]
    pub relevance: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConflictDto {
    pub group_id: String,
    pub key: String,
    pub scope_type: String,
    pub scope_id: String,
    pub status: String,
    pub winner_id: Option<String>,
    pub resolution_note: Option<String>,
    pub entries: Vec<EntryDto>,
}

#[derive(Debug, Deserialize)]
pub struct SaveResponse {
    pub ok: bool,
    pub id: String,
    #[serde(default)]
    pub deduplicated: bool,
    pub conflict: Option<ConflictDto>,
}

#[derive(Debug, Deserialize)]
pub struct RecallResponse {
    pub ok: bool,
    pub results: Vec<EntryDto>,
    pub engine: String,
}

#[derive(Debug, Deserialize)]
pub struct ListResponse {
    pub ok: bool,
    pub results: Vec<EntryDto>,
}

#[derive(Debug, Deserialize)]
pub struct GetResponse {
    pub ok: bool,
    pub entry: EntryDto,
}

#[derive(Debug, Deserialize)]
pub struct DeleteResponse {
    pub ok: bool,
    pub deleted: bool,
    pub mode: String,
    #[serde(default)]
    pub already_deleted: bool,
}

#[derive(Debug, Deserialize)]
pub struct ConflictsResponse {
    pub ok: bool,
    #[serde(default)]
    pub open_conflicts: Vec<ConflictDto>,
    pub conflict: Option<ConflictDto>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveResponse {
    pub ok: bool,
    pub conflict: ConflictDto,
}

#[derive(Debug, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub service: String,
    pub version: String,
    #[serde(default)]
    pub search_backend: Option<String>,
    #[serde(default)]
    pub db_state: Option<String>,
    #[serde(default)]
    pub quarantined_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StatusResponse {
    pub ok: bool,
    pub stats: serde_json::Value,
    pub counters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[allow(dead_code)]
    ok: bool,
    code: String,
    message: String,
}

/// Propuesta de guardado visible al usuario. NUNCA se persiste sin
/// /memoria confirmar (gate 2 de ADR-058).
#[derive(Debug, Clone)]
pub struct PendingProposal {
    pub content: String,
    pub key: Option<String>,
    pub scope_type: String,
    pub scope_id: String,
    pub source_reference: String,
    pub idempotency_key: String,
}

/// Errores del cliente, mapeados desde los MG_<AREA>_<NN> del contrato.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// Backend no descubrible o conexión rechazada (memoria no disponible).
    Unavailable(String),
    /// Presupuesto de tiempo agotado (visible y recuperable).
    Timeout,
    /// 401 MG_AUTH_01.
    Auth,
    /// 422 MG_WRITE_01 — escritura sin confirmación (fail-closed).
    NotConfirmed,
    /// 400 MG_VALID/MG_SCOPE/MG_HTTP_00 (fail-closed).
    InvalidPayload(String),
    /// 404 MG_GET/MG_DEL/MG_CONF_01.
    NotFound(String),
    /// 413 MG_HTTP_13.
    TooLarge,
    /// 503 MG_DB_02 — único error retryable del contrato.
    DbBusy,
    /// 503 MG_DB_03 — base en cuarentena: memoria no disponible + aviso.
    DbQuarantined(String),
    /// 409/500/otros del contrato.
    Server(String),
    /// Respuesta que no cumple el contrato.
    Protocol(String),
}

impl MemoryError {
    pub fn retryable(&self) -> bool {
        matches!(self, MemoryError::DbBusy)
    }

    /// Mensaje para el usuario, sin datos sensibles.
    pub fn user_message(&self) -> String {
        match self {
            MemoryError::Unavailable(_) => {
                "memoria no disponible (sidecar apagado). Nexum sigue funcionando \
                 sin memoria; reintentá con /memoria on"
                    .into()
            }
            MemoryError::Timeout => "la memoria tardó demasiado (timeout); reintentá".into(),
            MemoryError::Auth => "token de memoria inválido (reiniciá Nexum)".into(),
            MemoryError::NotConfirmed => {
                "escritura rechazada: falta confirmación explícita".into()
            }
            MemoryError::InvalidPayload(m) => format!("pedido inválido: {m}"),
            MemoryError::NotFound(m) => format!("no encontrado: {m}"),
            MemoryError::TooLarge => "contenido demasiado grande (> 256 KiB)".into(),
            MemoryError::DbBusy => "memoria ocupada, reintentá en un momento".into(),
            MemoryError::DbQuarantined(m) => format!(
                "memoria no disponible: base en cuarentena. {m} — usá /memoria reset \
                 para crear una base nueva (la aislada se preserva)"
            ),
            MemoryError::Server(m) => format!("error del gateway: {m}"),
            MemoryError::Protocol(m) => format!("respuesta fuera de contrato: {m}"),
        }
    }
}

pub(crate) fn map_error(status: u16, body: &str) -> MemoryError {
    let parsed: Option<ErrorBody> = serde_json::from_str(body).ok();
    let (code, message) = parsed
        .map(|e| (e.code, e.message))
        .unwrap_or_else(|| (format!("HTTP_{status}"), String::new()));
    match code.as_str() {
        "MG_AUTH_01" => MemoryError::Auth,
        "MG_WRITE_01" => MemoryError::NotConfirmed,
        "MG_HTTP_13" => MemoryError::TooLarge,
        "MG_DB_02" => MemoryError::DbBusy,
        "MG_DB_03" => MemoryError::DbQuarantined(message),
        c if c.starts_with("MG_VALID") || c.starts_with("MG_SCOPE") || c == "MG_HTTP_00" => {
            MemoryError::InvalidPayload(message)
        }
        c if c == "MG_GET_01" || c == "MG_DEL_01" || c == "MG_CONF_01" => {
            MemoryError::NotFound(message)
        }
        _ => MemoryError::Server(format!("{code}: {message}")),
    }
}
