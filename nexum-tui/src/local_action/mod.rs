//! Fast path de acciones locales determinísticas (NCP-LOCAL-ACTION-FAST-PATH-001).
//!
//! Owner: Runtime + Security + Tools. NO el Hormiguero (que solo puede
//! detectar/etiquetar intención, jamás ejecutar). Una acción local sencilla
//! (crear una carpeta) NO debe escalar a un modelo pago ni gastar tokens:
//! se reconoce con un parser determinístico (sin LLM, sin provider, sin shell),
//! se valida contra path traversal/symlink escape, exige aprobación (HITL) con
//! fingerprint, y se ejecuta con la API de filesystem de Rust revalidando el
//! path justo antes de escribir. Resultado real, sin false completion.
//!
//! Alcance RC-2: SOLO `CreateDirectory`. Borrar/mover/ejecutar quedan fuera.

use std::path::{Path, PathBuf};

mod exec;
mod parse;
pub mod pending;

pub use exec::{ExecOutcome, LocalActionError};
pub use parse::parse;

/// Longitud máxima de un nombre de carpeta (límite de la mayoría de FS).
const MAX_NAME_LEN: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalActionKind {
    CreateDirectory,
}

/// Una acción local candidata, ya con la base resuelta (Desktop/Documents/
/// workspace) pero AÚN NO ejecutada ni aprobada.
#[derive(Debug, Clone)]
pub struct LocalAction {
    pub kind: LocalActionKind,
    /// Nombre de la carpeta, ya normalizado y validado (sin separadores).
    pub name: String,
    /// Directorio base resuelto (existente).
    pub base: PathBuf,
    /// Etiqueta humana del destino ("escritorio", "documentos", "proyecto").
    pub base_label: String,
}

impl LocalAction {
    /// Path efectivo que se crearía. NO lo canonicaliza (la carpeta no existe
    /// todavía); la validación de seguridad usa `validate`.
    pub fn effective_path(&self) -> PathBuf {
        self.base.join(&self.name)
    }

    /// Texto de preview para el usuario (path exacto).
    pub fn preview(&self) -> String {
        format!(
            "crear la carpeta «{}» en {} → {}",
            self.name,
            self.base_label,
            self.effective_path().display()
        )
    }

    /// Fingerprint determinístico (SPEC-SECURITY-001): ata la aprobación a la
    /// acción exacta + path efectivo. Un cambio de nombre/base/kind invalida.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(format!("{:?}", self.kind).as_bytes());
        h.update([0u8]);
        // canonicaliza la base para el fingerprint (resuelve symlinks del padre).
        let canon_base = self.base.canonicalize().unwrap_or_else(|_| self.base.clone());
        h.update(canon_base.to_string_lossy().as_bytes());
        h.update([0u8]);
        h.update(self.name.as_bytes());
        format!("{:x}", h.finalize())
    }

    /// Validación de seguridad (Security). Rechaza traversal, paths absolutos,
    /// separadores y symlink escape. Se corre al proponer Y otra vez en
    /// `execute` (revalidación TOCTOU).
    pub fn validate(&self) -> Result<(), LocalActionError> {
        if self.name.is_empty() {
            return Err(LocalActionError::InvalidName("nombre vacío".into()));
        }
        if self.name.len() > MAX_NAME_LEN {
            return Err(LocalActionError::InvalidName("nombre demasiado largo".into()));
        }
        // Sin separadores ni traversal ni nombres peligrosos.
        if self.name.contains('/')
            || self.name.contains('\\')
            || self.name.contains('\0')
            || self.name == "."
            || self.name == ".."
            || self.name.contains("..")
        {
            return Err(LocalActionError::InvalidName(format!(
                "nombre no permitido: {:?}",
                self.name
            )));
        }
        if Path::new(&self.name).is_absolute() {
            return Err(LocalActionError::InvalidName("no se permite un path absoluto".into()));
        }
        // La base debe existir y ser un directorio real (no un symlink que
        // escape). Canonicalizamos y verificamos que el target siga dentro.
        let canon_base = self
            .base
            .canonicalize()
            .map_err(|e| LocalActionError::BaseUnavailable(format!("{}: {e}", self.base.display())))?;
        if !canon_base.is_dir() {
            return Err(LocalActionError::BaseUnavailable(format!(
                "{} no es un directorio",
                canon_base.display()
            )));
        }
        // El path efectivo debe quedar directamente bajo la base canónica.
        let target = canon_base.join(&self.name);
        if target.parent() != Some(canon_base.as_path()) {
            return Err(LocalActionError::PathEscape(target.display().to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "local_action_test.rs"]
mod local_action_test;

/// Resultado del fast path de acciones locales para el flujo de voz/texto.
#[derive(Debug)]
pub enum FastPathOutcome {
    /// Se ejecutó tras confirmación: mensaje para TTS/TUI + si creó o ya existía.
    Executed(String),
    /// El usuario canceló: cero cambios.
    Cancelled(String),
    /// Se propuso una acción (HITL): preview + pedido de confirmación. NO ejecutó.
    Proposed(String),
    /// Se detectó una acción pero es inválida/insegura: mensaje honesto, cero cambios.
    Rejected(String),
    /// No es una acción local: seguir el flujo normal (router/runtime).
    NotLocal,
}

/// Fast path determinístico para el flujo de voz/texto. Maneja el HITL de dos
/// pasos (proponer → confirmar) con estado en `state_dir`. NUNCA llama a un
/// provider ni gasta tokens. Devuelve `NotLocal` si el transcript no es una
/// acción local (para que el runtime lo procese normalmente).
pub fn dispatch(transcript: &str, workspace: &std::path::Path, state_dir: &std::path::Path) -> FastPathOutcome {
    // 1. ¿Hay una acción pendiente de confirmación?
    if let Some(p) = pending::load(state_dir) {
        if pending::is_affirmative(transcript) {
            let action = p.to_action();
            // Binding: el fingerprint recomputado debe coincidir con el aprobado.
            if action.fingerprint() != p.fingerprint {
                pending::clear(state_dir);
                return FastPathOutcome::Rejected(
                    "El pedido cambió desde que lo propuse; cancelé por seguridad. Repetilo si querés.".into(),
                );
            }
            pending::clear(state_dir);
            return match action.execute() {
                Ok(ExecOutcome::Created(path)) => {
                    FastPathOutcome::Executed(format!("Listo, creé la carpeta «{}» en {}. Ruta: {path}", action.name, action.base_label))
                }
                Ok(ExecOutcome::AlreadyExisted(path)) => {
                    FastPathOutcome::Executed(format!("La carpeta «{}» ya existía en {}. No cambié nada. Ruta: {path}", action.name, action.base_label))
                }
                Err(e) => FastPathOutcome::Rejected(format!("No pude crear la carpeta: {e}. No cambié nada.")),
            };
        }
        if pending::is_negative(transcript) {
            pending::clear(state_dir);
            return FastPathOutcome::Cancelled("Cancelado. No creé ninguna carpeta.".into());
        }
        // Cualquier otra cosa: se abandona la pendiente (cambio de tema).
        pending::clear(state_dir);
    }

    // 2. ¿Es una nueva acción local?
    match parse(transcript, workspace) {
        Some(action) => match action.validate() {
            Ok(()) => {
                if pending::save(state_dir, &action).is_err() {
                    return FastPathOutcome::Rejected("No pude preparar la acción (estado no escribible).".into());
                }
                FastPathOutcome::Proposed(format!(
                    "Voy a {}. ¿Lo confirmás? Decí «sí» para crearla o «no» para cancelar.",
                    action.preview()
                ))
            }
            Err(e) => FastPathOutcome::Rejected(format!("No puedo hacer eso de forma segura: {e}.")),
        },
        None => FastPathOutcome::NotLocal,
    }
}
