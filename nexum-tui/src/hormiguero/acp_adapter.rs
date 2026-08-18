//! Conversión tipada entre el contrato del Hormiguero y ACP.
//!
//! No usa JSON como intermediario. Los campos que no existen en el envelope
//! fuente (sesión, thread, workspace y selección runtime) se reciben mediante
//! un contexto explícito capturado en el mismo turno.

use std::collections::BTreeMap;

use nexum_acp::task::{
    EvidencePolicy, ExecutionBudgetV1, OutputFormat, TaskEnvelopeV1, TaskEnvelopeVersion,
    TaskPriority, TaskRisk, TaskSource,
};
use thiserror::Error;

use super::{StableRouteDecision, StableTaskClassification, TaskEnvelope};

#[derive(Debug, Clone)]
pub struct EnvelopeConversionContext {
    pub session_id: String,
    pub thread_id: String,
    pub workspace: String,
    pub selected_provider: String,
    pub selected_model: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EnvelopeConversionError {
    #[error("missing required task envelope field: {0}")]
    MissingRequired(&'static str),
    #[error("unsupported task envelope value for {field}: {value}")]
    UnsupportedValue { field: &'static str, value: String },
    #[error("stable task envelope cannot allow tools")]
    ToolsNotAllowed,
    #[error("source field has no safe typed ACP representation: {0}")]
    UnsupportedSourceField(&'static str),
}

fn required(value: &str, field: &'static str) -> Result<String, EnvelopeConversionError> {
    let value = value.trim();
    if value.is_empty() {
        Err(EnvelopeConversionError::MissingRequired(field))
    } else {
        Ok(value.to_string())
    }
}

/// Campo por campo:
///
/// | source | target | conversión | requerido | default |
/// | --- | --- | --- | --- | --- |
/// | envelope_id | envelope_id | copia | sí | no |
/// | source | source | enum validado | sí | no |
/// | user_intent | objective | copia | sí | no |
/// | normalized_request | user_input | copia | sí | no |
/// | context.session/thread/workspace | campos homónimos | copia | sí | no |
/// | constraints | constraints | copia | no | vacío |
/// | safety_notes | constraints + risk | copia; eleva risk a medium | no | vacío/low |
/// | allowed_tools | allowed_tools | sólo vacío | sí | vacío |
/// | required_output_format | output_format | enum validado | sí | no |
/// | relevant_context_summary/selected_project_context | — | sin equivalente
/// tipado seguro: fail-closed si llegan poblados | no | vacío |
/// | redaction_report/metrics/disallowed_tools | sanitized_metadata | sólo
/// contadores no sensibles | no | vacío |
/// | identidades/decisión/clase/provider/model | sanitized_metadata | copia
/// tipada a valores canónicos | sí | no |
///
/// `evidence_refs` y `success_criteria` quedan vacíos porque el contrato fuente
/// no contiene equivalentes tipados. La evidencia no es requerida para este
/// flujo conversacional. Prioridad normal y budget estable son propiedades del
/// perfil, no inferencias sobre el texto.
pub fn to_acp_task_envelope(
    source: &TaskEnvelope,
    context: &EnvelopeConversionContext,
) -> Result<TaskEnvelopeV1, EnvelopeConversionError> {
    if !source.allowed_tools.is_empty() {
        return Err(EnvelopeConversionError::ToolsNotAllowed);
    }
    if !source.relevant_context_summary.trim().is_empty() {
        return Err(EnvelopeConversionError::UnsupportedSourceField(
            "relevant_context_summary",
        ));
    }
    if !source.selected_project_context.is_empty() {
        return Err(EnvelopeConversionError::UnsupportedSourceField(
            "selected_project_context",
        ));
    }

    let source_kind = match required(&source.source, "source")?
        .to_ascii_lowercase()
        .as_str()
    {
        "tui" => TaskSource::Tui,
        other => {
            return Err(EnvelopeConversionError::UnsupportedValue {
                field: "source",
                value: other.to_string(),
            });
        }
    };
    let output_format = match required(&source.required_output_format, "required_output_format")?
        .to_ascii_lowercase()
        .as_str()
    {
        "text" => OutputFormat::Text,
        "markdown" | "md" => OutputFormat::Markdown,
        "json" => OutputFormat::Json,
        other => {
            return Err(EnvelopeConversionError::UnsupportedValue {
                field: "required_output_format",
                value: other.to_string(),
            });
        }
    };
    let route = match source.route_decision {
        StableRouteDecision::OneShot => "one_shot",
        StableRouteDecision::RejectedByPolicy => "rejected_by_policy",
        StableRouteDecision::Unspecified => {
            return Err(EnvelopeConversionError::MissingRequired("route_decision"));
        }
    };
    let classification = match source.task_classification {
        StableTaskClassification::Simple => "simple",
        StableTaskClassification::Advanced => "advanced",
        StableTaskClassification::Unspecified => {
            return Err(EnvelopeConversionError::MissingRequired(
                "task_classification",
            ));
        }
    };

    let mut metadata = BTreeMap::new();
    for (key, value) in [
        ("trace_id", required(&source.trace_id, "trace_id")?),
        ("turn_id", required(&source.turn_id, "turn_id")?),
        ("request_id", required(&source.request_id, "request_id")?),
        ("route_decision", route.to_string()),
        ("task_classification", classification.to_string()),
        (
            "selected_provider",
            required(&context.selected_provider, "selected_provider")?,
        ),
        (
            "selected_model",
            required(&context.selected_model, "selected_model")?,
        ),
    ] {
        metadata.insert(key.to_string(), value);
    }
    if !source.escalation_reason.trim().is_empty() {
        metadata.insert(
            "escalation_reason".to_string(),
            source.escalation_reason.trim().to_string(),
        );
    }
    metadata.insert("classifier".to_string(), "hormiguero-fastpath".to_string());
    metadata.insert("confidence".to_string(), source.confidence.to_string());
    if !source.disallowed_tools.is_empty() {
        metadata.insert(
            "disallowed_tools_count".to_string(),
            source.disallowed_tools.len().to_string(),
        );
    }
    if let Some(redaction) = &source.redaction_report {
        metadata.insert(
            "redacted_values_count".to_string(),
            redaction.secrets_redacted.to_string(),
        );
        metadata.insert(
            "paths_normalized".to_string(),
            redaction.paths_normalized.to_string(),
        );
        metadata.insert(
            "dropped_sections_count".to_string(),
            redaction.dropped_sections.len().to_string(),
        );
    }
    if let Some(metrics) = &source.metrics {
        metadata.insert(
            "naive_size_bytes".to_string(),
            metrics.naive_size_bytes.to_string(),
        );
        metadata.insert(
            "envelope_size_bytes".to_string(),
            metrics.envelope_size_bytes.to_string(),
        );
        if let Some(ratio) = metrics.envelope_naive_ratio {
            metadata.insert("envelope_naive_ratio".to_string(), ratio.to_string());
        }
    }

    let mut constraints = source.constraints.clone();
    constraints.extend(source.safety_notes.iter().cloned());

    Ok(TaskEnvelopeV1 {
        version: TaskEnvelopeVersion::V1,
        envelope_id: required(&source.envelope_id, "envelope_id")?,
        source: source_kind,
        objective: required(&source.user_intent, "user_intent")?,
        user_input: required(&source.normalized_request, "normalized_request")?,
        session_id: required(&context.session_id, "session_id")?,
        thread_id: required(&context.thread_id, "thread_id")?,
        workspace: Some(required(&context.workspace, "workspace")?),
        constraints,
        allowed_tools: Vec::new(),
        evidence_refs: Vec::new(),
        success_criteria: Vec::new(),
        output_format,
        execution_budget: ExecutionBudgetV1 {
            wall_time_ms: Some(90_000),
            max_tool_calls: Some(0),
            max_iterations: Some(1),
            max_depth: Some(0),
            max_tokens: None,
            max_cost_microusd: None,
        },
        evidence_policy: EvidencePolicy {
            require_evidence: false,
            minimum_evidence_refs: 0,
            allow_unverified_output: true,
        },
        priority: TaskPriority::Normal,
        risk: if source.safety_notes.is_empty() {
            TaskRisk::Low
        } else {
            TaskRisk::Medium
        },
        sanitized_metadata: nexum_acp::task::sanitize_metadata(metadata),
    })
}

#[cfg(test)]
#[path = "acp_adapter_test.rs"]
mod tests;
