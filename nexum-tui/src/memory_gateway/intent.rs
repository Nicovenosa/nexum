//! Detección determinística de solicitudes explícitas de guardado
//! (FASE 7 de M-3): frases del estilo "recordá que X". Conservador: el
//! mensaje debe EMPEZAR con el patrón — la conversación normal pasa de
//! largo. 0 tokens, sin LLM.

/// Prefijos reconocidos (minúsculas, con y sin tilde).
const PREFIXES: &[&str] = &[
    "recordá que ",
    "recorda que ",
    "recordame que ",
    "recordáme que ",
    "acordate que ",
    "acordáte que ",
    "guardá en memoria que ",
    "guarda en memoria que ",
    "guardá en memoria ",
    "guarda en memoria ",
];

/// Devuelve el contenido a recordar (casing original) si el mensaje es
/// una solicitud explícita de guardado.
pub fn parse_save_intent(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();
    for p in PREFIXES {
        if lower.starts_with(p) {
            let contenido = trimmed[p.len()..].trim();
            if !contenido.is_empty() {
                return Some(contenido.to_string());
            }
        }
    }
    None
}

/// Key determinística opcional: primera palabra significativa del
/// contenido, para que dos hechos sobre lo mismo colisionen y disparen
/// la detección de contradicciones (D-12). Conservador: solo si el
/// contenido empieza con "trabajo/vivo/uso/prefiero/mi ..." — hechos
/// personales estables típicos.
pub fn derive_key(contenido: &str) -> Option<String> {
    let lower = contenido.to_lowercase();
    for stable in ["trabajo ", "vivo ", "uso ", "prefiero ", "mi "] {
        if lower.starts_with(stable) {
            let palabras: Vec<&str> = lower.split_whitespace().take(2).collect();
            return Some(palabras.join("-"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_save_intent_frases_y_conversacion_normal() {
        assert_eq!(
            parse_save_intent("Recordá que trabajo en Nexum"),
            Some("trabajo en Nexum".to_string())
        );
        assert_eq!(
            parse_save_intent("acordate que el parcial es el lunes"),
            Some("el parcial es el lunes".to_string())
        );
        assert_eq!(
            parse_save_intent("guardá en memoria que uso Arch"),
            Some("uso Arch".to_string())
        );
        assert_eq!(parse_save_intent("hola cómo andás"), None);
        assert_eq!(parse_save_intent("me recordás qué era un socket?"), None);
        assert_eq!(parse_save_intent("recordá que "), None);
    }

    #[test]
    fn test_derive_key_hechos_estables() {
        assert_eq!(
            derive_key("trabajo en Nexum"),
            Some("trabajo-en".to_string())
        );
        assert_eq!(derive_key("el parcial es el lunes"), None);
    }
}
