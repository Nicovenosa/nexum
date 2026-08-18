use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};

use crate::transport::types::HostPrincipal;

use super::model::{
    from_millis, next_after, ContinuationCapability, CronJob, CronJobSpec, CronRun, CronRunStatus,
    PendingInteraction, PendingInteractionSpec, PendingInteractionStatus,
};

/// SQLite-backed durable store. Its schema is intentionally separate from the
/// agent ThreadStore: scheduled work references an existing thread by ID and
/// never owns conversations itself.
pub struct SqliteCronStore {
    pool: SqlitePool,
}

impl SqliteCronStore {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("crear directorio cron: {}", parent.display()))?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .pragma("journal_mode", "WAL")
            .pragma("synchronous", "NORMAL")
            .pragma("foreign_keys", "ON");
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS cron_schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at_ms INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
        let applied: Option<i64> =
            sqlx::query_scalar("SELECT version FROM cron_schema_migrations WHERE version = 1")
                .fetch_optional(&self.pool)
                .await?;
        if applied.is_none() {
            let mut transaction = self.pool.begin().await?;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS cron_jobs (
                 id TEXT PRIMARY KEY,
                target_thread_id TEXT NOT NULL,
                schedule TEXT NOT NULL,
                prompt TEXT NOT NULL,
                enabled INTEGER NOT NULL,
                next_run_at_ms INTEGER NOT NULL,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            )",
            )
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS cron_runs (
                 id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                target_thread_id TEXT NOT NULL,
                scheduled_for_ms INTEGER NOT NULL,
                status TEXT NOT NULL,
                started_at_ms INTEGER,
                finished_at_ms INTEGER,
                error TEXT,
                UNIQUE(job_id, scheduled_for_ms),
                FOREIGN KEY (job_id) REFERENCES cron_jobs(id) ON DELETE CASCADE
            )",
            )
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_cron_jobs_due
             ON cron_jobs(enabled, next_run_at_ms ASC)",
            )
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_cron_runs_status
             ON cron_runs(status, scheduled_for_ms ASC)",
            )
            .execute(&mut *transaction)
            .await?;
            sqlx::query("INSERT INTO cron_schema_migrations(version, applied_at_ms) VALUES(1, ?1)")
                .bind(Utc::now().timestamp_millis())
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
        }

        let result_migration: Option<i64> =
            sqlx::query_scalar("SELECT version FROM cron_schema_migrations WHERE version = 2")
                .fetch_optional(&self.pool)
                .await?;
        if result_migration.is_none() {
            let mut transaction = self.pool.begin().await?;
            sqlx::query("ALTER TABLE cron_runs ADD COLUMN result TEXT")
                .execute(&mut *transaction)
                .await?;
            sqlx::query("INSERT INTO cron_schema_migrations(version, applied_at_ms) VALUES(2, ?1)")
                .bind(Utc::now().timestamp_millis())
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
        }
        let interactions_migration: Option<i64> =
            sqlx::query_scalar("SELECT version FROM cron_schema_migrations WHERE version = 3")
                .fetch_optional(&self.pool)
                .await?;
        if interactions_migration.is_none() {
            let mut transaction = self.pool.begin().await?;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS cron_pending_interactions (
                    id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    job_id TEXT NOT NULL,
                    target_thread_id TEXT NOT NULL,
                    context_json TEXT NOT NULL,
                    status TEXT NOT NULL,
                    continuation_capability TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    expires_at_ms INTEGER NOT NULL,
                    resolved_at_ms INTEGER,
                    resolution_note TEXT,
                    FOREIGN KEY (run_id) REFERENCES cron_runs(id) ON DELETE CASCADE
                )",
            )
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_cron_pending_interactions_state
                 ON cron_pending_interactions(status, expires_at_ms ASC)",
            )
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_cron_pending_interactions_target
                 ON cron_pending_interactions(target_thread_id, created_at_ms ASC)",
            )
            .execute(&mut *transaction)
            .await?;
            sqlx::query("INSERT INTO cron_schema_migrations(version, applied_at_ms) VALUES(3, ?1)")
                .bind(Utc::now().timestamp_millis())
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
        }
        let owner_migration: Option<i64> =
            sqlx::query_scalar("SELECT version FROM cron_schema_migrations WHERE version = 4")
                .fetch_optional(&self.pool)
                .await?;
        if owner_migration.is_none() {
            let mut transaction = self.pool.begin().await?;
            sqlx::query("ALTER TABLE cron_jobs ADD COLUMN owner_principal TEXT")
                .execute(&mut *transaction)
                .await?;
            sqlx::query("ALTER TABLE cron_pending_interactions ADD COLUMN owner_principal TEXT")
                .execute(&mut *transaction)
                .await?;
            sqlx::query("INSERT INTO cron_schema_migrations(version, applied_at_ms) VALUES(4, ?1)")
                .bind(Utc::now().timestamp_millis())
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
        }
        Ok(())
    }

    pub async fn create_job(&self, spec: CronJobSpec) -> Result<CronJob> {
        let job = CronJob::new(spec, Utc::now())?;
        sqlx::query(
            "INSERT INTO cron_jobs(
                 id, target_thread_id, owner_principal, schedule, prompt, enabled, next_run_at_ms, created_at_ms, updated_at_ms
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&job.id)
        .bind(&job.target_thread_id)
        .bind(job.owner_principal.as_ref().map(HostPrincipal::as_str))
        .bind(&job.schedule)
        .bind(&job.prompt)
        .bind(job.enabled as i64)
        .bind(job.next_run_at.timestamp_millis())
        .bind(job.created_at.timestamp_millis())
        .bind(job.updated_at.timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(job)
    }

    pub async fn list_jobs(&self) -> Result<Vec<CronJob>> {
        let rows = sqlx::query_as::<_, JobRow>(
            "SELECT id, target_thread_id, owner_principal, schedule, prompt, enabled, next_run_at_ms, created_at_ms, updated_at_ms
             FROM cron_jobs ORDER BY created_at_ms ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(JobRow::into_job).collect()
    }

    pub async fn get_job(&self, id: &str) -> Result<Option<CronJob>> {
        let row = sqlx::query_as::<_, JobRow>(
            "SELECT id, target_thread_id, owner_principal, schedule, prompt, enabled, next_run_at_ms, created_at_ms, updated_at_ms
             FROM cron_jobs WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(JobRow::into_job).transpose()
    }

    /// Claims all due jobs transactionally and advances their schedule before
    /// execution. A unique `(job_id, scheduled_for_ms)` run row prevents a tick
    /// or recovery pass from creating duplicate occurrences.
    pub async fn claim_due(&self, now: DateTime<Utc>, limit: i64) -> Result<Vec<CronRun>> {
        let mut transaction = self.pool.begin().await?;
        let jobs = sqlx::query_as::<_, JobRow>(
            "SELECT id, target_thread_id, owner_principal, schedule, prompt, enabled, next_run_at_ms, created_at_ms, updated_at_ms
             FROM cron_jobs
             WHERE enabled = 1 AND next_run_at_ms <= ?1
             ORDER BY next_run_at_ms ASC, id ASC
             LIMIT ?2",
        )
        .bind(now.timestamp_millis())
        .bind(limit.max(1))
        .fetch_all(&mut *transaction)
        .await?;

        let mut runs = Vec::with_capacity(jobs.len());
        for row in jobs {
            let job = row.into_job()?;
            let scheduled_for = job.next_run_at;
            let next_run_at = next_after(&job.schedule, scheduled_for)?;
            let updated = sqlx::query(
                "UPDATE cron_jobs SET next_run_at_ms = ?1, updated_at_ms = ?2
                 WHERE id = ?3 AND enabled = 1 AND next_run_at_ms = ?4",
            )
            .bind(next_run_at.timestamp_millis())
            .bind(now.timestamp_millis())
            .bind(&job.id)
            .bind(scheduled_for.timestamp_millis())
            .execute(&mut *transaction)
            .await?;
            if updated.rows_affected() == 0 {
                continue;
            }
            let run = CronRun {
                id: uuid::Uuid::now_v7().to_string(),
                job_id: job.id,
                target_thread_id: job.target_thread_id,
                scheduled_for,
                status: CronRunStatus::Queued,
                started_at: None,
                finished_at: None,
                result: None,
                error: None,
            };
            sqlx::query(
                "INSERT INTO cron_runs(
                    id, job_id, target_thread_id, scheduled_for_ms, status, started_at_ms, finished_at_ms, error
                ) VALUES(?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL)",
            )
            .bind(&run.id)
            .bind(&run.job_id)
            .bind(&run.target_thread_id)
            .bind(run.scheduled_for.timestamp_millis())
            .bind(run.status.as_str())
            .execute(&mut *transaction)
            .await?;
            runs.push(run);
        }
        transaction.commit().await?;
        Ok(runs)
    }

    pub async fn mark_run_running(&self, id: &str, started_at: DateTime<Utc>) -> Result<()> {
        sqlx::query(
            "UPDATE cron_runs SET status = ?1, started_at_ms = ?2, finished_at_ms = NULL, error = NULL
             WHERE id = ?3 AND status = ?4",
        )
        .bind(CronRunStatus::Running.as_str())
        .bind(started_at.timestamp_millis())
        .bind(id)
        .bind(CronRunStatus::Queued.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_run_succeeded(
        &self,
        id: &str,
        finished_at: DateTime<Utc>,
        result: Option<String>,
    ) -> Result<()> {
        self.finish_run(id, CronRunStatus::Succeeded, finished_at, result, None)
            .await
    }

    pub async fn mark_run_failed(
        &self,
        id: &str,
        finished_at: DateTime<Utc>,
        error: String,
    ) -> Result<()> {
        self.finish_run(id, CronRunStatus::Failed, finished_at, None, Some(error))
            .await
    }

    pub async fn mark_run_failed_needs_user(
        &self,
        id: &str,
        finished_at: DateTime<Utc>,
        error: String,
    ) -> Result<()> {
        self.finish_run(
            id,
            CronRunStatus::FailedNeedsUser,
            finished_at,
            None,
            Some(error),
        )
        .await
    }

    async fn finish_run(
        &self,
        id: &str,
        status: CronRunStatus,
        finished_at: DateTime<Utc>,
        result: Option<String>,
        error: Option<String>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE cron_runs SET status = ?1, finished_at_ms = ?2, result = ?3, error = ?4
              WHERE id = ?5 AND status = ?6",
        )
        .bind(status.as_str())
        .bind(finished_at.timestamp_millis())
        .bind(result)
        .bind(error)
        .bind(id)
        .bind(CronRunStatus::Running.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Requeues runs left in `running` state by a process exit. Completed and
    /// failed occurrences are never retried by this basic recovery policy. A
    /// run with any durable interaction is terminal: restarting it could run a
    /// tool after an approval that cannot continue the original agent.
    pub async fn recover_interrupted_runs(&self) -> Result<Vec<CronRun>> {
        let now = Utc::now();
        sqlx::query(
            "UPDATE cron_runs
             SET status = ?1, finished_at_ms = ?2, error = ?3
             WHERE status = ?4
               AND EXISTS (
                    SELECT 1 FROM cron_pending_interactions
                    WHERE cron_pending_interactions.run_id = cron_runs.id
               )",
        )
        .bind(CronRunStatus::FailedNeedsUser.as_str())
        .bind(now.timestamp_millis())
        .bind("interacción cron durable encontrada durante recuperación")
        .bind(CronRunStatus::Running.as_str())
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "UPDATE cron_runs SET status = ?1, started_at_ms = NULL
             WHERE status = ?2",
        )
        .bind(CronRunStatus::Queued.as_str())
        .bind(CronRunStatus::Running.as_str())
        .execute(&self.pool)
        .await?;
        self.queued_runs().await
    }

    pub async fn queued_runs(&self) -> Result<Vec<CronRun>> {
        self.runs_by_status(CronRunStatus::Queued).await
    }

    pub async fn get_run(&self, id: &str) -> Result<Option<CronRun>> {
        let row = sqlx::query_as::<_, RunRow>(
            "SELECT id, job_id, target_thread_id, scheduled_for_ms, status, started_at_ms, finished_at_ms, result, error
             FROM cron_runs WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(RunRow::into_run).transpose()
    }

    pub async fn list_runs(&self) -> Result<Vec<CronRun>> {
        let rows = sqlx::query_as::<_, RunRow>(
            "SELECT id, job_id, target_thread_id, scheduled_for_ms, status, started_at_ms, finished_at_ms, result, error
             FROM cron_runs ORDER BY scheduled_for_ms ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(RunRow::into_run).collect()
    }

    /// Records a headless interaction before the broker rejects it. A newer
    /// interaction for the same target supersedes any unresolved older record.
    pub async fn create_pending_interaction(
        &self,
        spec: PendingInteractionSpec,
    ) -> Result<PendingInteraction> {
        if spec.run_id.trim().is_empty()
            || spec.job_id.trim().is_empty()
            || spec.target_thread_id.trim().is_empty()
        {
            anyhow::bail!("la interacción pendiente requiere run, job y target válidos");
        }
        let now = Utc::now();
        let context_json = serde_json::to_string(&spec.context)?;
        let owner_principal: Option<String> =
            sqlx::query_scalar("SELECT owner_principal FROM cron_jobs WHERE id = ?1")
                .bind(&spec.job_id)
                .fetch_optional(&self.pool)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!("no se encontró el job de la interacción pendiente")
                })?;
        let interaction = PendingInteraction {
            id: uuid::Uuid::now_v7().to_string(),
            run_id: spec.run_id,
            job_id: spec.job_id,
            target_thread_id: spec.target_thread_id,
            owner_principal: owner_principal.map(HostPrincipal::new).transpose()?,
            context: spec.context,
            status: PendingInteractionStatus::Pending,
            continuation_capability: ContinuationCapability::Unsupported,
            created_at: now,
            expires_at: spec.expires_at,
            resolved_at: None,
            resolution_note: None,
        };
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE cron_pending_interactions
             SET status = ?1, resolved_at_ms = ?2, resolution_note = ?3
             WHERE target_thread_id = ?4 AND status = ?5",
        )
        .bind(PendingInteractionStatus::Superseded.as_str())
        .bind(now.timestamp_millis())
        .bind("superada por una interacción cron más reciente")
        .bind(&interaction.target_thread_id)
        .bind(PendingInteractionStatus::Pending.as_str())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO cron_pending_interactions(
                 id, run_id, job_id, target_thread_id, owner_principal, context_json, status,
                 continuation_capability, created_at_ms, expires_at_ms, resolved_at_ms,
                 resolution_note
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL)",
        )
        .bind(&interaction.id)
        .bind(&interaction.run_id)
        .bind(&interaction.job_id)
        .bind(&interaction.target_thread_id)
        .bind(
            interaction
                .owner_principal
                .as_ref()
                .map(HostPrincipal::as_str),
        )
        .bind(context_json)
        .bind(interaction.status.as_str())
        .bind(interaction.continuation_capability.as_str())
        .bind(interaction.created_at.timestamp_millis())
        .bind(interaction.expires_at.timestamp_millis())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(interaction)
    }

    /// Marks every unresolved interaction whose expiry passed as terminal.
    pub async fn sweep_expired(&self, now: DateTime<Utc>) -> Result<u64> {
        let updated = sqlx::query(
            "UPDATE cron_pending_interactions
             SET status = ?1, resolved_at_ms = ?2, resolution_note = ?3
             WHERE status = ?4 AND expires_at_ms <= ?5",
        )
        .bind(PendingInteractionStatus::Expired.as_str())
        .bind(now.timestamp_millis())
        .bind("la interacción pendiente venció")
        .bind(PendingInteractionStatus::Pending.as_str())
        .bind(now.timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected())
    }

    pub async fn get_pending_interaction(
        &self,
        id: &str,
        target_thread_id: &str,
    ) -> Result<Option<PendingInteraction>> {
        let row = sqlx::query_as::<_, PendingInteractionRow>(
            "SELECT id, run_id, job_id, target_thread_id, owner_principal, context_json, status,
                    continuation_capability, created_at_ms, expires_at_ms, resolved_at_ms,
                    resolution_note
             FROM cron_pending_interactions
             WHERE id = ?1 AND target_thread_id = ?2",
        )
        .bind(id)
        .bind(target_thread_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(PendingInteractionRow::into_interaction).transpose()
    }

    pub async fn list_pending_interactions(
        &self,
        target_thread_id: Option<&str>,
    ) -> Result<Vec<PendingInteraction>> {
        let rows = if let Some(target_thread_id) = target_thread_id {
            sqlx::query_as::<_, PendingInteractionRow>(
                "SELECT id, run_id, job_id, target_thread_id, owner_principal, context_json, status,
                        continuation_capability, created_at_ms, expires_at_ms, resolved_at_ms,
                        resolution_note
                 FROM cron_pending_interactions
                 WHERE status = ?1 AND target_thread_id = ?2
                 ORDER BY created_at_ms ASC, id ASC",
            )
            .bind(PendingInteractionStatus::Pending.as_str())
            .bind(target_thread_id)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, PendingInteractionRow>(
                "SELECT id, run_id, job_id, target_thread_id, owner_principal, context_json, status,
                        continuation_capability, created_at_ms, expires_at_ms, resolved_at_ms,
                        resolution_note
                 FROM cron_pending_interactions
                 WHERE status = ?1 ORDER BY created_at_ms ASC, id ASC",
            )
            .bind(PendingInteractionStatus::Pending.as_str())
            .fetch_all(&self.pool)
            .await?
        };
        rows.into_iter()
            .map(PendingInteractionRow::into_interaction)
            .collect()
    }

    pub async fn resolve_pending_interaction(
        &self,
        id: &str,
        target_thread_id: &str,
        status: PendingInteractionStatus,
        note: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<PendingInteraction> {
        if !matches!(
            status,
            PendingInteractionStatus::Approved | PendingInteractionStatus::Rejected
        ) {
            anyhow::bail!("solo se puede aprobar o rechazar una interacción pendiente");
        }
        let updated = sqlx::query(
            "UPDATE cron_pending_interactions
             SET status = ?1, resolved_at_ms = ?2, resolution_note = ?3
             WHERE id = ?4 AND target_thread_id = ?5 AND status = ?6",
        )
        .bind(status.as_str())
        .bind(now.timestamp_millis())
        .bind(note)
        .bind(id)
        .bind(target_thread_id)
        .bind(PendingInteractionStatus::Pending.as_str())
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() != 1 {
            anyhow::bail!("la interacción no está pendiente o no pertenece al target indicado");
        }
        self.get_pending_interaction(id, target_thread_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("la interacción resuelta no pudo recuperarse"))
    }

    async fn runs_by_status(&self, status: CronRunStatus) -> Result<Vec<CronRun>> {
        let rows = sqlx::query_as::<_, RunRow>(
            "SELECT id, job_id, target_thread_id, scheduled_for_ms, status, started_at_ms, finished_at_ms, result, error
             FROM cron_runs WHERE status = ?1 ORDER BY scheduled_for_ms ASC, id ASC",
        )
        .bind(status.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(RunRow::into_run).collect()
    }
}

#[derive(sqlx::FromRow)]
struct JobRow {
    id: String,
    target_thread_id: String,
    owner_principal: Option<String>,
    schedule: String,
    prompt: String,
    enabled: i64,
    next_run_at_ms: i64,
    created_at_ms: i64,
    updated_at_ms: i64,
}

impl JobRow {
    fn into_job(self) -> Result<CronJob> {
        Ok(CronJob {
            id: self.id,
            target_thread_id: self.target_thread_id,
            owner_principal: self.owner_principal.map(HostPrincipal::new).transpose()?,
            schedule: self.schedule,
            prompt: self.prompt,
            enabled: self.enabled != 0,
            next_run_at: from_millis(self.next_run_at_ms)?,
            created_at: from_millis(self.created_at_ms)?,
            updated_at: from_millis(self.updated_at_ms)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct RunRow {
    id: String,
    job_id: String,
    target_thread_id: String,
    scheduled_for_ms: i64,
    status: String,
    started_at_ms: Option<i64>,
    finished_at_ms: Option<i64>,
    result: Option<String>,
    error: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PendingInteractionRow {
    id: String,
    run_id: String,
    job_id: String,
    target_thread_id: String,
    owner_principal: Option<String>,
    context_json: String,
    status: String,
    continuation_capability: String,
    created_at_ms: i64,
    expires_at_ms: i64,
    resolved_at_ms: Option<i64>,
    resolution_note: Option<String>,
}

impl PendingInteractionRow {
    fn into_interaction(self) -> Result<PendingInteraction> {
        Ok(PendingInteraction {
            id: self.id,
            run_id: self.run_id,
            job_id: self.job_id,
            target_thread_id: self.target_thread_id,
            owner_principal: self.owner_principal.map(HostPrincipal::new).transpose()?,
            context: serde_json::from_str(&self.context_json)?,
            status: PendingInteractionStatus::parse(&self.status)?,
            continuation_capability: ContinuationCapability::parse(&self.continuation_capability)?,
            created_at: from_millis(self.created_at_ms)?,
            expires_at: from_millis(self.expires_at_ms)?,
            resolved_at: self.resolved_at_ms.map(from_millis).transpose()?,
            resolution_note: self.resolution_note,
        })
    }
}

impl RunRow {
    fn into_run(self) -> Result<CronRun> {
        Ok(CronRun {
            id: self.id,
            job_id: self.job_id,
            target_thread_id: self.target_thread_id,
            scheduled_for: from_millis(self.scheduled_for_ms)?,
            status: CronRunStatus::parse(&self.status)?,
            started_at: self.started_at_ms.map(from_millis).transpose()?,
            finished_at: self.finished_at_ms.map(from_millis).transpose()?,
            result: self.result,
            error: self.error,
        })
    }
}
