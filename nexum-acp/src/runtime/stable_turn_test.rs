use super::*;
use crate::task::{
    EvidencePolicy, ExecutionBudgetV1, OutputFormat, TaskEnvelopeV1, TaskEnvelopeVersion,
    TaskPriority, TaskRisk, TaskSource,
};
use std::collections::BTreeMap;

#[test]
fn stable_policy_has_non_relaxable_limits() {
    let policy = StableTurnPolicy::stable();
    assert_eq!(policy.max_model_requests(), 1);
    assert_eq!(policy.max_agent_iterations(), 1);
    assert_eq!(policy.max_provider_retries(), 0);
    assert_eq!(policy.max_tool_calls(), 0);
    assert_eq!(policy.max_mcp_initializations(), 0);
    assert_eq!(policy.max_nested_agents(), 0);
    assert_eq!(policy.max_stream_reconnects(), 0);
    assert_eq!(policy.max_automatic_continuations(), 0);
    assert_eq!(policy.total_deadline(), Duration::from_secs(90));
    assert_eq!(policy.no_progress_deadline(), Duration::from_secs(30));
}

#[test]
fn stable_profile_uses_one_shot_retry_config() {
    // Behavioral proof lives in nexum-agent's retry RED/GREEN regression. This
    // assertion proves the runtime policy is the real consumer of that config.
    let _ = StableTurnPolicy::stable().retry_config();
}

fn envelope(decision: &str, classification: &str) -> TaskEnvelopeV1 {
    TaskEnvelopeV1 {
        version: TaskEnvelopeVersion::V1,
        envelope_id: "env-1".into(),
        source: TaskSource::Tui,
        objective: "objetivo".into(),
        user_input: "entrada".into(),
        session_id: "session".into(),
        thread_id: "thread".into(),
        workspace: Some("/tmp".into()),
        constraints: vec![],
        allowed_tools: vec![],
        evidence_refs: vec![],
        success_criteria: vec![],
        output_format: OutputFormat::Text,
        execution_budget: ExecutionBudgetV1::default(),
        evidence_policy: EvidencePolicy {
            require_evidence: false,
            minimum_evidence_refs: 0,
            allow_unverified_output: true,
        },
        priority: TaskPriority::Normal,
        risk: TaskRisk::Low,
        sanitized_metadata: BTreeMap::from([
            ("trace_id".into(), "trace".into()),
            ("turn_id".into(), "turn".into()),
            ("request_id".into(), "request".into()),
            ("route_decision".into(), decision.into()),
            ("task_classification".into(), classification.into()),
            ("selected_provider".into(), "OpenAI".into()),
            ("selected_model".into(), "model-a".into()),
        ]),
    }
}

fn provider() -> LlmProvider {
    LlmProvider::OpenAi {
        api_key: "test".into(),
        base_url: "https://example.invalid/v1".into(),
        model: "model-a".into(),
        thinking: None,
    }
}

#[test]
fn acp_uses_envelope_decision_without_reclassification() {
    let policy = StableTurnPolicy::stable();
    assert_eq!(
        policy
            .validate_envelope(&envelope("one_shot", "simple"), &provider())
            .unwrap(),
        StableFlow::OneShot
    );
    assert_eq!(
        policy
            .validate_envelope(&envelope("rejected_by_policy", "advanced"), &provider())
            .unwrap(),
        StableFlow::RejectedByPolicy
    );
}

#[test]
fn selected_provider_and_model_are_preserved() {
    let mut changed = envelope("one_shot", "simple");
    changed
        .sanitized_metadata
        .insert("selected_model".into(), "other-model".into());
    assert!(matches!(
        StableTurnPolicy::stable().validate_envelope(&changed, &provider()),
        Err(StablePolicyError::ProviderModelMismatch { .. })
    ));
}

#[test]
fn missing_stable_envelope_fails_closed() {
    assert_eq!(
        StableTurnPolicy::stable().require_envelope(None),
        Err(StablePolicyError::MissingEnvelope)
    );
}
