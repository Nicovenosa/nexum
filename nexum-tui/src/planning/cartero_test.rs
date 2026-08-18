//! Tests del Cartero: scopes de menor privilegio, redacción, límite de tamaño,
//! exclusión de contexto no requerido, provenance.

use super::*;
use crate::planning::types::{BudgetWire, PlanEnvelopeV1, PlanStepV1};

fn step(id: &str, capability: &str, action: &str) -> PlanStepV1 {
    PlanStepV1 {
        id: id.into(),
        action: action.into(),
        capability: capability.into(),
        risk_class: "low".into(),
        required_approval: false,
        dependencies: vec![],
        expected_evidence: vec![],
    }
}

fn envelope() -> PlanEnvelopeV1 {
    PlanEnvelopeV1 {
        schema_version: 1,
        plan_id: "plan-x".into(),
        task_id: "task-x".into(),
        task_class: "code".into(),
        risk_class: "low".into(),
        required_approval: false,
        ordered_steps: vec![],
        expected_evidence: vec!["tests pasan".into()],
        budgets: BudgetWire::default(),
        deadline_ms: 1000,
        stop_conditions: vec![],
        provenance: "hormiguero-planning/pol-1".into(),
        validation_status: "validated".into(),
        policy_version: "pol-1".into(),
        refine_iterations: 0,
    }
}

#[test]
fn test_scopes_menor_privilegio() {
    assert_eq!(scope_for_capability("read"), vec!["fs:read"]);
    assert_eq!(scope_for_capability("write"), vec!["fs:read", "fs:write"]);
    assert_eq!(scope_for_capability("bash"), vec!["proc:exec"]);
    assert_eq!(scope_for_capability("ANALYZE"), vec!["ctx:read"]); // kind genérico → menor privilegio
    assert_eq!(scope_for_capability("MUTATE"), vec!["ctx:read"]); // kind desconocido → conservador
}

#[test]
fn test_contexto_excluye_prompt_crudo() {
    let ctx = build_step_context(&step("s1", "read", "leer el archivo"), &envelope(), "PROMPT CRUDO DEL USUARIO");
    assert!(!ctx.payload.contains("PROMPT CRUDO"), "el prompt crudo NO viaja al worker");
    assert!(ctx.excluded_fields.contains(&"raw_user_prompt".to_string()));
    assert_eq!(ctx.provenance, "hormiguero-planning/pol-1#s1");
}

#[test]
fn test_redaccion_de_secretos() {
    let ctx = build_step_context(
        &step("s2", "read", "usar api_key=sk-ABCDEF1234567890abcdef para conectar"),
        &envelope(),
        "",
    );
    assert!(ctx.secrets_redacted, "detectó y redactó el secreto");
    assert!(!ctx.payload.contains("sk-ABCDEF1234567890abcdef"), "el secreto no queda en claro");
}

#[test]
fn test_limite_de_tamano() {
    let big = "a".repeat(MAX_CONTEXT_BYTES * 2);
    let ctx = build_step_context(&step("s3", "read", &big), &envelope(), "");
    assert!(ctx.size_bytes <= MAX_CONTEXT_BYTES, "contexto acotado");
}
