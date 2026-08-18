//! MemoryGateway — cliente del contrato de memoria honesta mínima
//! (SPEC-MEMORY-001 · ADR-058 · CHANGE-RUNTIME-001).
//!
//! El runtime conoce SOLO: DTOs, contrato HTTP loopback, errores
//! versionados, timeouts, feature flag y lifecycle client. Jamás
//! sqlite3, schema interno, clases Python ni packages/core.
//!
//! Flag `NEXUM_MEMORY` OFF por defecto (D-13): sin flag, cero
//! lecturas/escrituras y el backend no es requerido.

pub mod client;
mod http;
pub mod intent;
pub mod inyeccion;
pub mod types;

pub use types::*;

/// Flag de entorno (mismo criterio que el sidecar: 1/true/on/yes).
pub fn env_flag_on() -> bool {
    matches!(
        std::env::var("NEXUM_MEMORY").unwrap_or_default().to_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// Estado UI de memoria por sesión de TUI: override on/off del usuario
/// (/memoria on|off) + propuesta pendiente de confirmación + conflicto
/// pendiente de resolución. Nada de esto persiste: la persistencia vive
/// exclusivamente detrás del gateway.
#[derive(Default)]
pub struct MemoryUiState {
    /// None = sin override (manda el flag). Some(false) = usuario desactivó.
    pub session_enabled: Option<bool>,
    /// Propuesta visible esperando /memoria confirmar (jamás se persiste sola).
    pub pending: Option<types::PendingProposal>,
    /// Conflicto mostrado esperando /memoria resolver.
    pub pending_conflict: Option<types::ConflictDto>,
}

/// Memoria efectiva: flag de entorno Y sin desactivación del usuario.
pub fn enabled(st: &MemoryUiState) -> bool {
    env_flag_on() && st.session_enabled.unwrap_or(true)
}

/// Scope user por defecto de la máquina (single-user, v0.1).
pub const USER_SCOPE_ID: &str = "local";

/// Scope project: nombre del directorio de trabajo actual.
pub fn project_scope_id() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "default".to_string())
}

#[cfg(test)]
#[path = "gateway_test.rs"]
mod gateway_test;

#[cfg(test)]
#[path = "e2e_test.rs"]
mod e2e_test;
