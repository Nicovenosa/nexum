use super::*;
use crate::hormiguero::{StableRouteDecision, StableTaskClassification};

fn source() -> TaskEnvelope {
    TaskEnvelope {
        envelope_id: "env-1".into(),
        trace_id: "trace-1".into(),
        turn_id: "turn-1".into(),
        request_id: "request-1".into(),
        route_decision: StableRouteDecision::OneShot,
        task_classification: StableTaskClassification::Simple,
        user_intent: "saludar".into(),
        normalized_request: "Hola".into(),
        required_output_format: "text".into(),
        source: "tui".into(),
        confidence: 1.0,
        ..TaskEnvelope::default()
    }
}

fn context() -> EnvelopeConversionContext {
    EnvelopeConversionContext {
        session_id: "session-1".into(),
        thread_id: "thread-1".into(),
        workspace: "/tmp/work".into(),
        selected_provider: "OpenAI".into(),
        selected_model: "model-a".into(),
    }
}

#[test]
fn task_envelope_conversion_preserves_required_fields() {
    let target = to_acp_task_envelope(&source(), &context()).unwrap();
    assert_eq!(target.envelope_id, "env-1");
    assert_eq!(target.objective, "saludar");
    assert_eq!(target.user_input, "Hola");
    assert_eq!(target.session_id, "session-1");
    assert_eq!(target.thread_id, "thread-1");
    assert_eq!(target.allowed_tools, Vec::<String>::new());
    assert_eq!(target.sanitized_metadata["route_decision"], "one_shot");
}

#[test]
fn task_envelope_conversion_rejects_missing_required_field() {
    let mut source = source();
    source.trace_id.clear();
    assert_eq!(
        to_acp_task_envelope(&source, &context()),
        Err(EnvelopeConversionError::MissingRequired("trace_id"))
    );
}

#[test]
fn selected_provider_and_model_survive_conversion() {
    let target = to_acp_task_envelope(&source(), &context()).unwrap();
    assert_eq!(target.sanitized_metadata["selected_provider"], "OpenAI");
    assert_eq!(target.sanitized_metadata["selected_model"], "model-a");
}

#[test]
fn trace_and_turn_identity_survive_conversion() {
    let target = to_acp_task_envelope(&source(), &context()).unwrap();
    assert_eq!(target.sanitized_metadata["trace_id"], "trace-1");
    assert_eq!(target.sanitized_metadata["turn_id"], "turn-1");
    assert_eq!(target.sanitized_metadata["request_id"], "request-1");
}

#[test]
fn policy_metadata_survives_conversion_without_sensitive_payloads() {
    let mut source = source();
    source.constraints.push("una restricción".into());
    source.safety_notes.push("revisar riesgo".into());
    source.disallowed_tools.push("Write".into());
    source.redaction_report = Some(crate::hormiguero::RedactionReport {
        secrets_redacted: 2,
        paths_normalized: 1,
        dropped_sections: vec!["credentials".into()],
    });
    source.metrics = Some(crate::hormiguero::EnvelopeMetrics {
        naive_size_bytes: 100,
        envelope_size_bytes: 50,
        envelope_naive_ratio: Some(0.5),
    });

    let target = to_acp_task_envelope(&source, &context()).unwrap();
    assert_eq!(
        target.constraints,
        ["una restricción".to_string(), "revisar riesgo".to_string()]
    );
    assert_eq!(target.risk, nexum_acp::task::TaskRisk::Medium);
    assert_eq!(target.sanitized_metadata["disallowed_tools_count"], "1");
    assert_eq!(target.sanitized_metadata["redacted_values_count"], "2");
    assert_eq!(target.sanitized_metadata["envelope_size_bytes"], "50");
    assert!(
        !target.sanitized_metadata.contains_key("credentials"),
        "redaction section names must not leak into metadata"
    );
}

#[test]
fn untyped_context_fails_closed_instead_of_being_dropped() {
    let mut source = source();
    source.relevant_context_summary = "contexto libre".into();
    assert_eq!(
        to_acp_task_envelope(&source, &context()),
        Err(EnvelopeConversionError::UnsupportedSourceField(
            "relevant_context_summary"
        ))
    );
}
