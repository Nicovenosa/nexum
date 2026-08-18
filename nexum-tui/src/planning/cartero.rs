//! Cartero (context broker) — Fase A. Construye el contexto MÍNIMO y TIPADO que
//! un Worker necesita para un paso del plan, bajo autoridad de Rust:
//!   - aplica scopes de menor privilegio por capability;
//!   - redacta secretos (reusa el redactor del runtime);
//!   - limita el tamaño (char-safe);
//!   - adjunta provenance;
//!   - EXCLUYE contexto no requerido (no incluye el prompt crudo del usuario);
//!   - evita datos privados por defecto (solo hashes/estructura en evidencia).
//!
//! Rust decide qué contexto se entrega; el Worker jamás pide más.

use super::types::{PlanEnvelopeV1, PlanStepV1};

/// Tope duro del contexto por paso (defensa contra unbounded_context).
pub const MAX_CONTEXT_BYTES: usize = 4096;

/// Contexto tipado de un paso — lo único que ve el Worker.
#[derive(Debug, Clone, PartialEq)]
pub struct StepContext {
    pub step_id: String,
    pub capability: String,
    /// Scopes de menor privilegio autorizados para esta capability.
    pub scope: Vec<String>,
    pub provenance: String,
    /// Payload redactado y acotado. JAMÁS secretos ni prompt crudo.
    pub payload: String,
    pub size_bytes: usize,
    /// True si la redacción cambió algo (hubo secretos removidos).
    pub secrets_redacted: bool,
    /// Campos de contexto excluidos por no ser requeridos (privacidad).
    pub excluded_fields: Vec<String>,
}

/// Scopes de menor privilegio para una capability. Modelo conservador: cada
/// capability abre el mínimo scope necesario y nada más.
pub fn scope_for_capability(capability: &str) -> Vec<String> {
    let c = capability.to_ascii_lowercase();
    match c.as_str() {
        "read" | "glob" | "grep" => vec!["fs:read".into()],
        "write" | "edit" => vec!["fs:read".into(), "fs:write".into()],
        "bash" | "shell" | "execute" => vec!["proc:exec".into()],
        "delete" => vec!["fs:write".into()],
        "network" | "webfetch" | "websearch" => vec!["net:read".into()],
        // kinds genéricos del planner (READ/ANALYZE/etc.) → solo lectura mínima.
        _ => vec!["ctx:read".into()],
    }
}

/// Trunca a nivel de carácter (nunca corta un char multibyte).
fn char_truncate(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        if out.len() + ch.len_utf8() > max_bytes {
            break;
        }
        out.push(ch);
    }
    out
}

/// Construye el contexto mínimo de un paso. `raw_input` se usa SOLO para derivar
/// evidencia (hash) — nunca se copia crudo al contexto (privacidad por defecto).
pub fn build_step_context(
    step: &PlanStepV1,
    envelope: &PlanEnvelopeV1,
    _raw_input: &str,
) -> StepContext {
    let scope = scope_for_capability(&step.capability);

    // El payload es la ACCIÓN del paso (estructura del plan), redactada. No se
    // incluye el prompt crudo del usuario ni contexto de otros pasos.
    let redacted = crate::ui::secret_redact::redact_secrets(&step.action);
    let secrets_redacted = redacted != step.action;
    let payload = char_truncate(&redacted, MAX_CONTEXT_BYTES);

    // Campos explícitamente excluidos (para trazabilidad de la minimización).
    let mut excluded_fields = vec!["raw_user_prompt".to_string()];
    if !envelope.expected_evidence.is_empty() {
        // La evidencia esperada del plan NO viaja al worker salvo la del paso.
        excluded_fields.push("plan_expected_evidence".to_string());
    }

    StepContext {
        step_id: step.id.clone(),
        capability: step.capability.clone(),
        scope,
        provenance: format!("{}#{}", envelope.provenance, step.id),
        size_bytes: payload.len(),
        secrets_redacted,
        payload,
        excluded_fields,
    }
}

#[cfg(test)]
#[path = "cartero_test.rs"]
mod cartero_test;
