//! Versioned task contracts and budget enforcement for ACP prompt requests.
//!
//! This module is deliberately pure: it does not construct providers or tools.
//! Callers record execution facts and forward the resulting structured events.

use std::{
    collections::{BTreeMap, HashMap},
    time::Instant,
};

use serde::{Deserialize, Deserializer, Serialize};
use tokio_util::sync::CancellationToken;

const MAX_METADATA_ENTRIES: usize = 64;
const MAX_METADATA_VALUE_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEnvelopeVersion {
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSource {
    Voice,
    Tui,
    Automation,
    Nocturno,
    Api,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Text,
    Markdown,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRisk {
    Low,
    Medium,
    High,
    Critical,
}

/// Limits declared by a task. Token and cost limits are observational until a
/// provider supplies reliable telemetry; they are never treated as enforced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBudgetV1 {
    pub wall_time_ms: Option<u64>,
    pub max_tool_calls: Option<u32>,
    pub max_iterations: Option<u32>,
    pub max_depth: Option<u32>,
    pub max_tokens: Option<u64>,
    pub max_cost_microusd: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidencePolicy {
    pub require_evidence: bool,
    pub minimum_evidence_refs: u32,
    pub allow_unverified_output: bool,
}

/// V1 ACP task envelope. It intentionally has no history/messages field: task
/// context is bounded to explicit evidence references and request input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskEnvelopeV1 {
    pub version: TaskEnvelopeVersion,
    pub envelope_id: String,
    pub source: TaskSource,
    pub objective: String,
    pub user_input: String,
    pub session_id: String,
    pub thread_id: String,
    pub workspace: Option<String>,
    pub constraints: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub success_criteria: Vec<String>,
    pub output_format: OutputFormat,
    pub execution_budget: ExecutionBudgetV1,
    pub evidence_policy: EvidencePolicy,
    pub priority: TaskPriority,
    pub risk: TaskRisk,
    #[serde(
        serialize_with = "serialize_sanitized_metadata",
        deserialize_with = "deserialize_sanitized_metadata"
    )]
    pub sanitized_metadata: BTreeMap<String, String>,
}

fn serialize_sanitized_metadata<S>(
    metadata: &BTreeMap<String, String>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    sanitize_metadata(metadata.clone()).serialize(serializer)
}

fn deserialize_sanitized_metadata<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    BTreeMap::<String, String>::deserialize(deserializer).map(sanitize_metadata)
}

/// Remove credential-like metadata and bound retained values before an envelope
/// can enter logs, events, or downstream execution contexts.
pub fn sanitize_metadata(metadata: BTreeMap<String, String>) -> BTreeMap<String, String> {
    metadata
        .into_iter()
        .filter(|(key, _)| !is_sensitive_metadata_key(key))
        .take(MAX_METADATA_ENTRIES)
        .map(|(key, value)| (key, value.chars().take(MAX_METADATA_VALUE_CHARS).collect()))
        .collect()
}

fn is_sensitive_metadata_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "authorization",
        "token",
        "secret",
        "password",
        "cookie",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetMetric {
    WallTime,
    ToolCalls,
    Iterations,
    Depth,
    Cancellation,
    Tokens,
    CostMicrousd,
    AllowedTools,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BudgetEnforcementStatus {
    NotConfigured,
    Enforced { limit: u64, observed: u64 },
    Exceeded { limit: u64, observed: u64 },
    Rejected,
    Cancelled,
    TelemetryUnavailable,
    TelemetryObserved { observed: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetEventV1 {
    pub version: TaskEnvelopeVersion,
    pub envelope_id: String,
    pub metric: BudgetMetric,
    pub status: BudgetEnforcementStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetViolation {
    Exceeded {
        metric: BudgetMetric,
        limit: u64,
        observed: u64,
    },
    ToolNotAllowed {
        tool_name: String,
    },
    Cancelled,
}

/// In-memory enforcement state for a single envelope execution.
///
/// The controller enforces only limits it can observe locally. Tokens and cost
/// remain telemetry statuses because ACP has no trustworthy provider telemetry
/// in this contracts-only layer.
pub struct BudgetEnforcer {
    envelope: TaskEnvelopeV1,
    started_at: Instant,
    tool_calls: u32,
    iterations: u32,
    depth: u32,
    statuses: HashMap<BudgetMetric, BudgetEnforcementStatus>,
    events: Vec<BudgetEventV1>,
}

impl BudgetEnforcer {
    pub fn new(envelope: TaskEnvelopeV1) -> Self {
        Self::new_at(envelope, Instant::now())
    }

    pub fn new_at(envelope: TaskEnvelopeV1, started_at: Instant) -> Self {
        let mut statuses = HashMap::new();
        for metric in [BudgetMetric::Tokens, BudgetMetric::CostMicrousd] {
            statuses.insert(metric, BudgetEnforcementStatus::TelemetryUnavailable);
        }
        Self {
            envelope,
            started_at,
            tool_calls: 0,
            iterations: 0,
            depth: 0,
            statuses,
            events: Vec::new(),
        }
    }

    pub fn events(&self) -> &[BudgetEventV1] {
        &self.events
    }

    pub fn take_events(&mut self) -> Vec<BudgetEventV1> {
        std::mem::take(&mut self.events)
    }

    pub fn status(&self, metric: BudgetMetric) -> BudgetEnforcementStatus {
        self.statuses
            .get(&metric)
            .cloned()
            .unwrap_or(BudgetEnforcementStatus::NotConfigured)
    }

    pub fn check_wall_time(&mut self) -> Result<(), BudgetViolation> {
        let Some(limit) = self.envelope.execution_budget.wall_time_ms else {
            return Ok(());
        };
        let observed = self
            .started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        self.enforce(BudgetMetric::WallTime, limit, observed)
    }

    /// Tool names are matched exactly so the allowlist has no case-folding
    /// ambiguity. An empty `allowed_tools` list permits no tools.
    pub fn record_tool_call(&mut self, tool_name: &str) -> Result<(), BudgetViolation> {
        if !self
            .envelope
            .allowed_tools
            .iter()
            .any(|allowed| allowed == tool_name)
        {
            self.emit(
                BudgetMetric::AllowedTools,
                BudgetEnforcementStatus::Rejected,
            );
            return Err(BudgetViolation::ToolNotAllowed {
                tool_name: tool_name.to_string(),
            });
        }
        let limit = self.envelope.execution_budget.max_tool_calls;
        let observed = self.tool_calls.saturating_add(1);
        self.increment(BudgetMetric::ToolCalls, limit, observed)?;
        self.tool_calls = observed;
        Ok(())
    }

    pub fn record_iteration(&mut self) -> Result<(), BudgetViolation> {
        let limit = self.envelope.execution_budget.max_iterations;
        let observed = self.iterations.saturating_add(1);
        self.increment(BudgetMetric::Iterations, limit, observed)?;
        self.iterations = observed;
        Ok(())
    }

    pub fn enter_depth(&mut self) -> Result<(), BudgetViolation> {
        let limit = self.envelope.execution_budget.max_depth;
        let observed = self.depth.saturating_add(1);
        self.increment(BudgetMetric::Depth, limit, observed)?;
        self.depth = observed;
        Ok(())
    }

    pub fn exit_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub fn check_cancelled(&mut self, cancelled: bool) -> Result<(), BudgetViolation> {
        if cancelled {
            self.emit(
                BudgetMetric::Cancellation,
                BudgetEnforcementStatus::Cancelled,
            );
            return Err(BudgetViolation::Cancelled);
        }
        Ok(())
    }

    /// Enforce cancellation against the same typed token used by ACP execution.
    pub fn check_cancellation_token(
        &mut self,
        cancellation: &CancellationToken,
    ) -> Result<(), BudgetViolation> {
        self.check_cancelled(cancellation.is_cancelled())
    }

    pub fn observe_tokens(&mut self, observed: Option<u64>) -> BudgetEnforcementStatus {
        self.observe_telemetry(BudgetMetric::Tokens, observed)
    }

    pub fn observe_cost_microusd(&mut self, observed: Option<u64>) -> BudgetEnforcementStatus {
        self.observe_telemetry(BudgetMetric::CostMicrousd, observed)
    }

    fn increment(
        &mut self,
        metric: BudgetMetric,
        limit: Option<u32>,
        observed: u32,
    ) -> Result<(), BudgetViolation> {
        match limit {
            Some(limit) => self.enforce(metric, u64::from(limit), u64::from(observed)),
            None => Ok(()),
        }
    }

    fn enforce(
        &mut self,
        metric: BudgetMetric,
        limit: u64,
        observed: u64,
    ) -> Result<(), BudgetViolation> {
        if observed > limit {
            self.emit(
                metric,
                BudgetEnforcementStatus::Exceeded { limit, observed },
            );
            return Err(BudgetViolation::Exceeded {
                metric,
                limit,
                observed,
            });
        }
        self.emit(
            metric,
            BudgetEnforcementStatus::Enforced { limit, observed },
        );
        Ok(())
    }

    fn observe_telemetry(
        &mut self,
        metric: BudgetMetric,
        observed: Option<u64>,
    ) -> BudgetEnforcementStatus {
        let status = observed.map_or(BudgetEnforcementStatus::TelemetryUnavailable, |observed| {
            BudgetEnforcementStatus::TelemetryObserved { observed }
        });
        self.emit(metric, status.clone());
        status
    }

    fn emit(&mut self, metric: BudgetMetric, status: BudgetEnforcementStatus) {
        self.statuses.insert(metric, status.clone());
        self.events.push(BudgetEventV1 {
            version: TaskEnvelopeVersion::V1,
            envelope_id: self.envelope.envelope_id.clone(),
            metric,
            status,
        });
    }
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
