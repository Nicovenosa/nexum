//! Estado de aprobación pendiente para el HITL por voz de dos pasos.
//! Una acción local propuesta se guarda con su fingerprint; una confirmación
//! posterior del usuario la ejecuta solo si el fingerprint coincide (binding).
//! Sin confirmación explícita: cero cambios (fail-closed).

use std::path::{Path, PathBuf};

use super::{LocalAction, LocalActionKind};

/// ¿El transcript es una confirmación afirmativa explícita?
pub fn is_affirmative(transcript: &str) -> bool {
    let first = first_token(transcript);
    matches!(
        first.as_str(),
        "sí" | "si" | "dale" | "confirmá" | "confirma" | "confirmo" | "hacelo"
            | "correcto" | "ok" | "okey" | "adelante" | "sip" | "obvio"
    )
}

/// ¿El transcript es una negación/cancelación explícita?
pub fn is_negative(transcript: &str) -> bool {
    // Cancelación clara: "no", "cancelá", "dejá" en los primeros dos tokens
    // (cubre "no", "no gracias", "mejor no", "cancelá eso").
    let toks: Vec<String> = transcript
        .trim()
        .to_lowercase()
        .split_whitespace()
        .take(2)
        .map(|t| t.trim_matches(|c: char| c.is_ascii_punctuation()).to_string())
        .collect();
    toks.iter().any(|t| matches!(t.as_str(),
        "no" | "cancelá" | "cancela" | "cancelar" | "dejá" | "deja" | "nope"))
}

/// Primer token en minúsculas, sin puntuación de bordes.
fn first_token(transcript: &str) -> String {
    transcript
        .trim()
        .to_lowercase()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| c.is_ascii_punctuation())
        .to_string()
}

/// Acción pendiente serializada (formato JSON plano, sin dependencias externas).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub kind: String,
    pub name: String,
    pub base: String,
    pub base_label: String,
    pub fingerprint: String,
}

impl Pending {
    pub fn from_action(a: &LocalAction) -> Self {
        Self {
            kind: format!("{:?}", a.kind),
            name: a.name.clone(),
            base: a.base.display().to_string(),
            base_label: a.base_label.clone(),
            fingerprint: a.fingerprint(),
        }
    }

    pub fn to_action(&self) -> LocalAction {
        LocalAction {
            kind: LocalActionKind::CreateDirectory,
            name: self.name.clone(),
            base: PathBuf::from(&self.base),
            base_label: self.base_label.clone(),
        }
    }

    fn serialize(&self) -> String {
        // JSON minimal escapado a mano (nombres controlados; evita dep serde acá).
        fn esc(s: &str) -> String {
            s.replace('\\', "\\\\").replace('"', "\\\"")
        }
        format!(
            "{{\"kind\":\"{}\",\"name\":\"{}\",\"base\":\"{}\",\"base_label\":\"{}\",\"fingerprint\":\"{}\"}}",
            esc(&self.kind), esc(&self.name), esc(&self.base), esc(&self.base_label), esc(&self.fingerprint)
        )
    }

    fn deserialize(s: &str) -> Option<Self> {
        let get = |key: &str| -> Option<String> {
            let needle = format!("\"{key}\":\"");
            let start = s.find(&needle)? + needle.len();
            let rest = &s[start..];
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => return Some(out),
                    '\\' => {
                        if let Some(n) = chars.next() {
                            out.push(n);
                        }
                    }
                    _ => out.push(c),
                }
            }
            None
        };
        Some(Pending {
            kind: get("kind")?,
            name: get("name")?,
            base: get("base")?,
            base_label: get("base_label")?,
            fingerprint: get("fingerprint")?,
        })
    }
}

fn pending_path(dir: &Path) -> PathBuf {
    dir.join("voice-pending-action.json")
}

/// Guarda una acción pendiente (proponer). Sobrescribe cualquier pendiente
/// anterior (solo una a la vez).
pub fn save(dir: &Path, a: &LocalAction) -> std::io::Result<()> {
    std::fs::write(pending_path(dir), Pending::from_action(a).serialize())
}

/// Carga la acción pendiente, si hay.
pub fn load(dir: &Path) -> Option<Pending> {
    let s = std::fs::read_to_string(pending_path(dir)).ok()?;
    Pending::deserialize(&s)
}

/// Descarta la pendiente (tras ejecutar, cancelar o cambiar de tema).
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_file(pending_path(dir));
}

#[cfg(test)]
#[path = "pending_test.rs"]
mod pending_test;
