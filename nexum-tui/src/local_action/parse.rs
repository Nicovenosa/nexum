//! Parser determinístico español para acciones locales (0 tokens, 0 LLM).
//! Conservador: solo dispara con frases explícitas de crear carpeta/directorio.

use std::path::{Path, PathBuf};

use super::{LocalAction, LocalActionKind};

/// Destinos reconocidos y su resolución de base.
enum Target {
    Desktop,
    Documents,
    Workspace,
}

/// Resuelve la base de un destino usando xdg-user-dir (con fallbacks seguros).
fn resolve_base(target: &Target, workspace: &Path) -> Option<(PathBuf, String)> {
    match target {
        Target::Workspace => Some((workspace.to_path_buf(), "el proyecto actual".into())),
        Target::Desktop => Some((xdg_dir("DESKTOP", &["Escritorio", "Desktop"])?, "el escritorio".into())),
        Target::Documents => Some((xdg_dir("DOCUMENTS", &["Documentos", "Documents"])?, "documentos".into())),
    }
}

/// xdg-user-dir <KEY>; si falla, prueba ~/<fallback> en orden.
fn xdg_dir(key: &str, fallbacks: &[&str]) -> Option<PathBuf> {
    if let Ok(out) = std::process::Command::new("xdg-user-dir").arg(key).output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                let pb = PathBuf::from(p);
                if pb.is_dir() {
                    return Some(pb);
                }
            }
        }
    }
    let home = std::env::var("HOME").ok()?;
    for f in fallbacks {
        let pb = PathBuf::from(&home).join(f);
        if pb.is_dir() {
            return Some(pb);
        }
    }
    None
}

/// Normaliza un nombre de carpeta capturado: recorta, quita comillas y
/// puntuación final. NO recorta separadores (los rechaza `validate`).
fn normalize_name(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c: char| c == '"' || c == '«' || c == '»' || c == '\'' || c == '.')
        .trim()
        .to_string()
}

/// Detecta el destino en el texto (después del nombre).
fn detect_target(lower: &str) -> Option<Target> {
    if lower.contains("escritorio") || lower.contains("desktop") {
        Some(Target::Desktop)
    } else if lower.contains("documento") || lower.contains("documents") {
        Some(Target::Documents)
    } else if lower.contains("proyecto") || lower.contains("directorio actual")
        || lower.contains("carpeta actual") || lower.contains("acá") || lower.contains("aca")
        || lower.contains("aquí") || lower.contains("aqui")
    {
        Some(Target::Workspace)
    } else {
        None
    }
}

/// Parsea un transcript. Devuelve Some(LocalAction) solo si es una intención
/// clara de crear carpeta/directorio con nombre y destino resolubles.
pub fn parse(transcript: &str, workspace: &Path) -> Option<LocalAction> {
    let lower = transcript.to_lowercase();
    // Verbo de creación + sustantivo carpeta/directorio. Tolerante a errores
    // comunes del ASR base en español ("creá"→"criá", "carpeta"→"capeta"):
    // el HITL muestra el path exacto antes de crear, así un transcript
    // imperfecto se ve y se corrige/cancela — nunca crea algo sorpresa.
    let has_verb = lower.contains("cre") || lower.contains("cri") || lower.contains("hac")
        || lower.contains("armá") || lower.contains("arma ");
    let has_noun = lower.contains("carpeta") || lower.contains("capeta")
        || lower.contains("carpeta") || lower.contains("directori");
    if !(has_verb && has_noun) {
        return None;
    }
    // Extraer el nombre: tras "llamada/llamado X", o tras "carpeta/directorio X".
    let name = extract_name(transcript, &lower)?;
    if name.is_empty() {
        return None;
    }
    let target = detect_target(&lower).unwrap_or(Target::Workspace);
    let (base, base_label) = resolve_base(&target, workspace)?;
    Some(LocalAction {
        kind: LocalActionKind::CreateDirectory,
        name,
        base,
        base_label,
    })
}

/// Palabras de destino/relleno que marcan el fin del nombre.
const STOP_WORDS: &[&str] = &[
    "en", "dentro", "sobre", "de", "del", "mi", "el", "la", "escritorio",
    "documentos", "proyecto", "acá", "aca", "aquí", "aqui", "por", "favor",
];

fn extract_name(orig: &str, lower: &str) -> Option<String> {
    // 1) "llamada/llamado <nombre>"
    for marker in ["llamada ", "llamado ", "de nombre ", "con nombre "] {
        if let Some(i) = lower.find(marker) {
            let start = i + marker.len();
            return Some(take_name(&orig[start..], &lower[start..]));
        }
    }
    // 2) "carpeta <nombre>" / "directorio <nombre>"
    for marker in ["carpeta ", "capeta ", "carpeta ", "directorio ", "directori "] {
        if let Some(i) = lower.find(marker) {
            let start = i + marker.len();
            let candidate = take_name(&orig[start..], &lower[start..]);
            // Evitar capturar "actual" u otra stop-word como nombre.
            if !candidate.is_empty() && !STOP_WORDS.contains(&candidate.to_lowercase().as_str()) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Toma el nombre desde el inicio del slice hasta la primera stop-word.
fn take_name(orig_tail: &str, lower_tail: &str) -> String {
    let words_orig: Vec<&str> = orig_tail.split_whitespace().collect();
    let words_lower: Vec<&str> = lower_tail.split_whitespace().collect();
    let mut taken: Vec<&str> = Vec::new();
    for (i, wl) in words_lower.iter().enumerate() {
        let clean = wl.trim_matches(|c: char| c.is_ascii_punctuation());
        if STOP_WORDS.contains(&clean) {
            break;
        }
        if let Some(wo) = words_orig.get(i) {
            taken.push(wo);
        }
        // Un solo token suele ser el nombre; permitir hasta 4 por nombres compuestos.
        if taken.len() >= 4 {
            break;
        }
    }
    super::parse::normalize_name(&taken.join(" "))
}
