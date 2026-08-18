use std::{collections::BTreeMap, time::Duration};

use serde_json::json;

use super::*;

fn make_envelope() -> TaskEnvelopeV1 {
    TaskEnvelopeV1 {
        version: TaskEnvelopeVersion::V1,
        envelope_id: "env-123".to_string(),
        source: TaskSource::Voice,
        objective: "Verificar contratos ACP".to_string(),
        user_input: "No exponer secretos".to_string(),
        session_id: "session-1".to_string(),
        thread_id: "thread-1".to_string(),
        workspace: Some("/tmp/project".to_string()),
        constraints: vec!["sin red".to_string()],
        allowed_tools: vec!["Read".to_string()],
        evidence_refs: vec!["docs/spec.md".to_string()],
        success_criteria: vec!["tests green".to_string()],
        output_format: OutputFormat::Markdown,
        execution_budget: ExecutionBudgetV1 {
            wall_time_ms: Some(10),
            max_tool_calls: Some(1),
            max_iterations: Some(2),
            max_depth: Some(1),
            max_tokens: Some(100),
            max_cost_microusd: Some(50),
        },
        evidence_policy: EvidencePolicy {
            require_evidence: true,
            minimum_evidence_refs: 1,
            allow_unverified_output: false,
        },
        priority: TaskPriority::High,
        risk: TaskRisk::Low,
        sanitized_metadata: BTreeMap::from([
            ("request_id".to_string(), "safe-123".to_string()),
            ("api_key".to_string(), "should-not-serialize".to_string()),
        ]),
    }
}

#[test]
fn test_task_envelope_serialization_sanitizes_metadata_and_omits_history() {
    let serialized = serde_json::to_value(make_envelope()).unwrap();

    assert_eq!(serialized["envelope_id"], "env-123");
    assert_eq!(serialized["source"], "voice");
    assert_eq!(serialized["sanitized_metadata"]["request_id"], "safe-123");
    assert!(serialized["sanitized_metadata"].get("api_key").is_none());
    assert!(serialized.get("history").is_none());
    assert!(serialized.get("messages").is_none());
}

#[test]
fn test_task_envelope_deserialization_rejects_history_payload() {
    let mut payload = serde_json::to_value(make_envelope()).unwrap();
    payload["history"] = json!(["unbounded history is forbidden"]);

    let result = serde_json::from_value::<TaskEnvelopeV1>(payload);

    assert!(
        result.is_err(),
        "el contrato no debe aceptar historial adjunto"
    );
}

#[test]
fn test_budget_enforcer_rejects_unallowed_tool_deterministically() {
    let mut enforcer = BudgetEnforcer::new(make_envelope());

    let error = enforcer.record_tool_call("Write").unwrap_err();

    assert_eq!(
        error,
        BudgetViolation::ToolNotAllowed {
            tool_name: "Write".to_string()
        }
    );
    assert_eq!(enforcer.events().len(), 1);
    assert_eq!(enforcer.events()[0].metric, BudgetMetric::AllowedTools);
    assert_eq!(
        enforcer.events()[0].status,
        BudgetEnforcementStatus::Rejected
    );
}

#[test]
fn test_budget_enforcer_enforces_wall_time_tool_calls_iterations_and_depth() {
    let envelope = make_envelope();
    let start = std::time::Instant::now() - Duration::from_millis(11);
    let mut enforcer = BudgetEnforcer::new_at(envelope, start);

    assert!(matches!(
        enforcer.check_wall_time(),
        Err(BudgetViolation::Exceeded {
            metric: BudgetMetric::WallTime,
            ..
        })
    ));
    assert!(enforcer.record_tool_call("Read").is_ok());
    assert!(matches!(
        enforcer.record_tool_call("Read"),
        Err(BudgetViolation::Exceeded {
            metric: BudgetMetric::ToolCalls,
            ..
        })
    ));
    assert!(enforcer.record_iteration().is_ok());
    assert!(enforcer.record_iteration().is_ok());
    assert!(matches!(
        enforcer.record_iteration(),
        Err(BudgetViolation::Exceeded {
            metric: BudgetMetric::Iterations,
            ..
        })
    ));
    assert!(enforcer.enter_depth().is_ok());
    assert!(matches!(
        enforcer.enter_depth(),
        Err(BudgetViolation::Exceeded {
            metric: BudgetMetric::Depth,
            ..
        })
    ));
}

#[test]
fn test_budget_enforcer_cancellation_and_telemetry_status_are_honest() {
    let mut enforcer = BudgetEnforcer::new(make_envelope());
    let cancellation = tokio_util::sync::CancellationToken::new();

    assert!(enforcer.check_cancellation_token(&cancellation).is_ok());
    cancellation.cancel();
    assert!(matches!(
        enforcer.check_cancellation_token(&cancellation),
        Err(BudgetViolation::Cancelled)
    ));
    assert_eq!(
        enforcer.status(BudgetMetric::Cancellation),
        BudgetEnforcementStatus::Cancelled
    );
    assert_eq!(
        enforcer.status(BudgetMetric::Tokens),
        BudgetEnforcementStatus::TelemetryUnavailable
    );
    assert_eq!(
        enforcer.observe_tokens(Some(90)),
        BudgetEnforcementStatus::TelemetryObserved { observed: 90 }
    );
    assert_eq!(
        enforcer.observe_cost_microusd(None),
        BudgetEnforcementStatus::TelemetryUnavailable
    );
}
