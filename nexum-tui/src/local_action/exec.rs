//! Ejecución segura de una acción local ya aprobada. Revalida el path justo
//! antes de escribir (TOCTOU) y usa la API de filesystem de Rust (no shell).

use super::{LocalAction, LocalActionKind};

#[derive(Debug, PartialEq, Eq)]
pub enum ExecOutcome {
    /// Carpeta creada en este path.
    Created(String),
    /// Ya existía (informado honestamente, no es un error).
    AlreadyExisted(String),
}

#[derive(Debug)]
pub enum LocalActionError {
    InvalidName(String),
    BaseUnavailable(String),
    PathEscape(String),
    Io(String),
}

impl std::fmt::Display for LocalActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LocalActionError::InvalidName(m) => write!(f, "nombre inválido: {m}"),
            LocalActionError::BaseUnavailable(m) => write!(f, "destino no disponible: {m}"),
            LocalActionError::PathEscape(m) => write!(f, "el path escapa del destino: {m}"),
            LocalActionError::Io(m) => write!(f, "error de filesystem: {m}"),
        }
    }
}

impl LocalAction {
    /// Ejecuta la acción. DEBE llamarse solo tras aprobación explícita del
    /// usuario. Revalida la seguridad (el FS pudo cambiar desde la aprobación)
    /// y crea la carpeta con `std::fs::create_dir` (no `create_dir_all`: la
    /// base ya existe; no se crean padres inesperados). Resultado real.
    pub fn execute(&self) -> Result<ExecOutcome, LocalActionError> {
        // Revalidación TOCTOU: el símil de la aprobación puede haber cambiado.
        self.validate()?;
        match self.kind {
            LocalActionKind::CreateDirectory => {
                let canon_base = self
                    .base
                    .canonicalize()
                    .map_err(|e| LocalActionError::BaseUnavailable(e.to_string()))?;
                let target = canon_base.join(&self.name);
                // Confirmar de nuevo que el target queda bajo la base canónica.
                if target.parent() != Some(canon_base.as_path()) {
                    return Err(LocalActionError::PathEscape(target.display().to_string()));
                }
                if target.exists() {
                    return Ok(ExecOutcome::AlreadyExisted(target.display().to_string()));
                }
                match std::fs::create_dir(&target) {
                    Ok(()) => Ok(ExecOutcome::Created(target.display().to_string())),
                    // Carrera: otro proceso la creó entre el exists() y ahora.
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        Ok(ExecOutcome::AlreadyExisted(target.display().to_string()))
                    }
                    Err(e) => Err(LocalActionError::Io(e.to_string())),
                }
            }
        }
    }
}
