//! Directivas de modelo por voz/texto — SOLO el parser local (0 tokens).
//!
//! La aplicación real del cambio vive en `acp_turn.rs`
//! (`VoiceTurnController` → ACP `session/set_config_option("model")`,
//! con aprobación pendiente para cambios Local→pago). Este módulo NO
//! aplica nada: traduce frases del usuario a una `ModelDirective` que
//! `intent_router` convierte en `VoiceRouteDecision::ModelDirective`.
//!
//! Historia: hasta F2.2 acá vivía un `ModelSwitcher` con un
//! `ModelControlPort` cuyo único impl productivo (`AcpModelControl`)
//! respondía siempre `Unavailable` (no existía transporte ACP). F2.3
//! implementó el transporte real en `acp_turn.rs` y esa maquinaria
//! quedó sin callers; se retiró en V-1 (2026-07-17). Jamás se escribe
//! `settings.json` directo desde voz: la escritura es del runtime vía ACP.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDirective {
    SwitchTo(String),
    ShowCurrent,
    PersistDefault,
    PreviousModel,
}

/// Parser local (0 tokens). Conservador: solo captura frases que hablan
/// de modelo/motor o verbos de cambio explícitos; la conversación normal
/// pasa de largo (y un target no resoluble sin la palabra "modelo"
/// también).
pub fn parse(phrase: &str) -> Option<ModelDirective> {
    let f = format!(" {} ", phrase.to_lowercase());
    let habla_de_modelo = f.contains("modelo") || f.contains("motor pago");
    if habla_de_modelo && (f.contains("anterior") || f.contains("de antes")) {
        return Some(ModelDirective::PreviousModel);
    }
    if habla_de_modelo
        && (f.contains("actual")
            || f.contains("mostrame")
            || f.contains("cuál")
            || f.contains("cual")
            || f.contains("estás usando")
            || f.contains("estas usando")
            || f.contains("qué modelo"))
    {
        return Some(ModelDirective::ShowCurrent);
    }
    if habla_de_modelo && (f.contains("predeterminado") || f.contains("por defecto")) {
        return Some(ModelDirective::PersistDefault);
    }
    for pat in [
        " cambiá a ",
        " cambia a ",
        " cambiá al ",
        " cambia al ",
        " usá ",
        " usa ",
        " pasate a ",
        " pásate a ",
        " pasá a ",
    ] {
        if let Some(i) = f.find(pat) {
            let tail = f[i + pat.len()..].trim();
            let tail = tail.strip_prefix("el modelo ").unwrap_or(tail);
            let tail = tail
                .strip_prefix("modelo ")
                .unwrap_or(tail)
                .trim_matches(|c: char| c.is_ascii_punctuation() || c == ' ');
            if !tail.is_empty() {
                return Some(ModelDirective::SwitchTo(tail.to_string()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frases_de_modelo_y_conversacion_normal() {
        assert_eq!(
            parse("cambiá al modelo claude"),
            Some(ModelDirective::SwitchTo("claude".into()))
        );
        assert_eq!(
            parse("qué modelo estás usando"),
            Some(ModelDirective::ShowCurrent)
        );
        assert_eq!(
            parse("dejá este modelo por defecto"),
            Some(ModelDirective::PersistDefault)
        );
        assert_eq!(
            parse("volvé al modelo anterior"),
            Some(ModelDirective::PreviousModel)
        );
        assert_eq!(parse("hola cómo andás"), None);
    }

    #[test]
    fn test_parse_switch_limpia_prefijos_y_puntuacion() {
        assert_eq!(
            parse("usá el modelo codex."),
            Some(ModelDirective::SwitchTo("codex".into()))
        );
        assert_eq!(
            parse("pasate a modelo glm"),
            Some(ModelDirective::SwitchTo("glm".into()))
        );
    }

    #[test]
    fn test_parse_target_vacio_no_dispara() {
        assert_eq!(parse("cambiá a "), None);
    }
}
