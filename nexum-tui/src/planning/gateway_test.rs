//! Tests del PlanningGateway: política de aprobación (autoridad Rust),
//! mapeo a PlanEnvelopeV1, scaffold, invariante de métricas.

use super::*;
use crate::planning::metrics::PlanningCounters;
use crate::planning::types::{BudgetWire, PlanStepWire, PlanWire};

fn sample_plan(risk: &str, tool: Option<&str>) -> PlanWire {
    PlanWire {
        schema_version: 1,
        task_class: "code".into(),
        route: "planning".into(),
        assumptions: vec![],
        steps: vec![PlanStepWire {
            id: "s1".into(),
            action: "escribir la función".into(),
            kind: "READ".into(),
            depends_on: vec![],
            evidence: vec!["diff".into()],
            tool: tool.map(|s| s.to_string()),
        }],
        expected_evidence: vec!["tests pasan".into()],
        risk: risk.into(),
        tool_intents: vec![],
        worker_hints: vec![],
        budget: BudgetWire {
            max_steps: 5,
            max_latency_ms: 30000,
            max_refine_iterations: 2,
        },
        stop_conditions: vec!["error irrecuperable".into()],
    }
}

#[test]
fn test_aprobacion_es_autoridad_de_rust() {
    // riesgo bajo + capability de lectura ⇒ sin aprobación
    assert!(!PlanningGateway::approval_for("low", "READ"));
    // capability de escritura ⇒ aprobación aunque el riesgo sea bajo
    assert!(PlanningGateway::approval_for("low", "write"));
    assert!(PlanningGateway::approval_for("low", "bash"));
    // riesgo elevado ⇒ aprobación aunque la capability sea de lectura
    assert!(PlanningGateway::approval_for("elevated", "READ"));
    assert!(PlanningGateway::approval_for("high", "READ"));
}

#[test]
fn test_encapsula_a_plan_envelope_v1() {
    let env = gateway().encapsulate(
        sample_plan("low", Some("write")),
        "validated",
        "plan_validated",
        "pol-1",
        0,
        "task-abc",
    );
    assert_eq!(env.schema_version, 1);
    assert_eq!(env.task_id, "task-abc");
    assert_eq!(env.risk_class, "low");
    assert_eq!(env.validation_status, "validated");
    assert!(env.plan_id.starts_with("plan-"));
    assert_eq!(env.ordered_steps.len(), 1);
    // el paso con tool=write ⇒ aprobación requerida (autoridad Rust)
    assert!(env.ordered_steps[0].required_approval);
    assert!(env.required_approval, "el envelope hereda aprobación de sus pasos");
    assert_eq!(env.ordered_steps[0].capability, "write");
    assert_eq!(env.deadline_ms, 30000);
    assert!(env.provenance.contains("hormiguero-planning"));
}

#[test]
fn test_scaffold_no_filtra_reasoning_y_es_estructural() {
    let env = gateway().encapsulate(
        sample_plan("low", None),
        "validated",
        "plan_validated",
        "pol-1",
        0,
        "task-xyz",
    );
    let s = env.execution_scaffold();
    assert!(s.contains("<nexum-plan"));
    assert!(s.contains("escribir la función"));
    assert!(s.contains("Rust conserva autoridad"));
    // capability por defecto = kind cuando no hay tool
    assert_eq!(env.ordered_steps[0].capability, "READ");
}

#[test]
fn test_invariante_metricas() {
    // consumed == generated - rejected, ignored_valid_plans == 0
    let c = PlanningCounters {
        planning_requested: 10,
        planning_generated: 8,
        planning_rejected: 3,
        planning_consumed: 5,
        planning_bypassed: 0,
        validator_failed: 3,
        plan_execution_completed: 5,
        plan_execution_failed: 0,
        ignored_valid_plans: 0,
    };
    assert!(c.invariant_holds(), "8 - 3 == 5");

    let bad = PlanningCounters {
        planning_generated: 8,
        planning_rejected: 3,
        planning_consumed: 4, // debería ser 5
        ..Default::default()
    };
    assert!(!bad.invariant_holds(), "violación detectada");

    let ignored = PlanningCounters {
        planning_generated: 5,
        planning_rejected: 0,
        planning_consumed: 5,
        ignored_valid_plans: 1, // plan válido ignorado ⇒ inválido
        ..Default::default()
    };
    assert!(!ignored.invariant_holds(), "ignored_valid_plans>0 rompe invariante");
}

/// Test LIVE (gateado por NEXUM_PLANNING_LIVE=1): el gateway REAL habla con el
/// sidecar REAL. Prueba end-to-end Planner→Validator→Rust-consume→Evidence sin
/// mocks. El wrapper (tests harness) publica hormiguero.port/token en
/// NEXUM_HORMIGUERO_RUNTIME_DIR y arranca el sidecar Python.
#[test]
fn test_live_gateway_contra_sidecar_real() {
    if std::env::var("NEXUM_PLANNING_LIVE").as_deref() != Ok("1") {
        return; // skip salvo corrida live explícita
    }
    let _guard = crate::hormiguero::bridge::test_env_lock();
    // El wrapper debe haber seteado el runtime_dir con los archivos de discovery.
    let dec = gateway().request_plan(
        "escribí una función que ordene una lista y agregá tests unitarios",
        "code",
        "low",
        true,
        "trace-live",
        "task-live",
    );
    match dec {
        super::PlanDecision::Validated(env) => {
            assert!(!env.ordered_steps.is_empty(), "plan con pasos");
            gateway().mark_consumed("trace-live", "task-live", &env.plan_id, &env.provenance);
            let (ok, fail) = crate::planning::evidence::verify_chain();
            assert!(ok >= 2, "evidencia persistida (plan_generated+validated)");
            assert!(fail.is_none(), "cadena de evidencia íntegra");
            let m = gateway().metrics().snapshot();
            assert!(m.planning_generated >= 1 && m.planning_consumed >= 1);
        }
        other => panic!("esperaba Validated del sidecar real, obtuve {other:?}"),
    }
}
