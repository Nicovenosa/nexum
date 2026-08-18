use std::str::FromStr;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use croner::Cron;
use nexum_agent::interaction::InteractionContext;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::transport::types::HostPrincipal;

/// Input persisted when a host registers a scheduled prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronJobSpec {
    pub target_thread_id: String,
    pub owner_principal: HostPrincipal,
    pub schedule: String,
    pub prompt: String,
}

/// Durable cron job. `target_thread_id` selects the session lane, not a new
/// conversation or a new thread store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub target_thread_id: String,
    /// `None` represents a record created before ownership migration and must
    /// never be authorized by a host.
    pub owner_principal: Option<HostPrincipal>,
    pub schedule: String,
    pub prompt: String,
    pub enabled: bool,
    pub next_run_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl CronJob {
    pub fn new(spec: CronJobSpec, now: DateTime<Utc>) -> Result<Self> {
        let now = from_millis(now.timestamp_millis())?;
        if spec.target_thread_id.trim().is_empty() {
            bail!("cron target_thread_id no puede estar vacío");
        }
        if spec.prompt.trim().is_empty() {
            bail!("cron prompt no puede estar vacío");
        }
        let next_run_at = next_after(&spec.schedule, now)?;
        Ok(Self {
            id: Uuid::now_v7().to_string(),
            target_thread_id: spec.target_thread_id,
            owner_principal: Some(spec.owner_principal),
            schedule: spec.schedule,
            prompt: spec.prompt,
            enabled: true,
            next_run_at,
            created_at: now,
            updated_at: now,
        })
    }
}

/// Lifecycle state persisted for every claimed schedule occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CronRunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    FailedNeedsUser,
}

impl CronRunStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::FailedNeedsUser => "failed_needs_user",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "failed_needs_user" => Ok(Self::FailedNeedsUser),
            _ => bail!("estado cron desconocido: {value}"),
        }
    }
}

/// Durable record of one scheduled occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronRun {
    pub id: String,
    pub job_id: String,
    pub target_thread_id: String,
    pub scheduled_for: DateTime<Utc>,
    pub status: CronRunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Final assistant text for a successful headless prompt, when present.
    pub result: Option<String>,
    pub error: Option<String>,
}

/// Headless cron interactions cannot be resumed: the agent has already stopped
/// before a human can resolve the durable record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContinuationCapability {
    Unsupported,
}

impl ContinuationCapability {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "unsupported" => Ok(Self::Unsupported),
            _ => bail!("capacidad de continuación desconocida: {value}"),
        }
    }
}

/// Lifecycle state for a durable interaction that was requested by a headless
/// cron occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PendingInteractionStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
    Cancelled,
    Superseded,
}

impl PendingInteractionStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Superseded => "superseded",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            "superseded" => Ok(Self::Superseded),
            _ => bail!("estado de interacción pendiente desconocido: {value}"),
        }
    }
}

/// Input captured when a headless execution needs user interaction.
#[derive(Debug, Clone)]
pub struct PendingInteractionSpec {
    pub run_id: String,
    pub job_id: String,
    pub target_thread_id: String,
    pub context: InteractionContext,
    pub expires_at: DateTime<Utc>,
}

/// Durable interaction record. Resolution is an audit action only: no
/// continuation capability is retained for the original execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInteraction {
    pub id: String,
    pub run_id: String,
    pub job_id: String,
    pub target_thread_id: String,
    /// `None` represents a record created before ownership migration and must
    /// never be authorized by a host.
    pub owner_principal: Option<HostPrincipal>,
    pub context: InteractionContext,
    pub status: PendingInteractionStatus,
    pub continuation_capability: ContinuationCapability,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolution_note: Option<String>,
}

pub(crate) fn next_after(schedule: &str, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let cron =
        Cron::from_str(schedule).map_err(|error| anyhow::anyhow!("cron inválido: {error}"))?;
    cron.iter_after(after)
        .next()
        .ok_or_else(|| anyhow::anyhow!("cron sin próxima ejecución: {schedule}"))
}

pub(crate) fn from_millis(value: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp_millis(value)
        .ok_or_else(|| anyhow::anyhow!("timestamp cron fuera de rango: {value}"))
}
