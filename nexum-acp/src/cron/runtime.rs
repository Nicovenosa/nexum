use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::future::join_all;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::{CronRun, CronRunExecutor, CronRunOutcome, SqliteCronStore};

/// Declares exactly what this vertical slice provides. In particular, hosts must
/// provide a prompt-context factory before scheduled prompts can run headlessly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CronCapabilities {
    pub durable_jobs: bool,
    pub durable_runs: bool,
    pub durable_pending_interactions: bool,
    pub fail_safely_interaction_policy: bool,
    pub interaction_continuation_supported: bool,
    pub sqlite_migrations: bool,
    pub single_scheduler_per_runtime: bool,
    pub session_lanes: bool,
    pub interrupted_run_recovery: bool,
    pub execute_prompt_adapter: bool,
    pub headless_execution: bool,
    pub requires_host_prompt_context: bool,
    pub acp_management_api: bool,
}

/// One process-local scheduler backed by a shared durable store.
pub struct CronRuntime {
    store: Arc<SqliteCronStore>,
    executor: Arc<dyn CronRunExecutor>,
    lanes: DashMap<String, Arc<Mutex<()>>>,
    scheduler_started: AtomicBool,
    shutdown: CancellationToken,
}

impl CronRuntime {
    pub fn new(store: Arc<SqliteCronStore>, executor: Arc<dyn CronRunExecutor>) -> Self {
        Self {
            store,
            executor,
            lanes: DashMap::new(),
            scheduler_started: AtomicBool::new(false),
            shutdown: CancellationToken::new(),
        }
    }

    pub const fn capabilities() -> CronCapabilities {
        CronCapabilities {
            durable_jobs: true,
            durable_runs: true,
            durable_pending_interactions: true,
            fail_safely_interaction_policy: true,
            interaction_continuation_supported: false,
            sqlite_migrations: true,
            single_scheduler_per_runtime: true,
            session_lanes: true,
            interrupted_run_recovery: true,
            execute_prompt_adapter: true,
            headless_execution: true,
            requires_host_prompt_context: true,
            acp_management_api: true,
        }
    }

    /// Starts the sole tick loop for this runtime. Calling it a second time is
    /// rejected instead of silently starting a competing scheduler.
    pub fn start(self: &Arc<Self>, tick_interval: Duration) -> Result<()> {
        if tick_interval.is_zero() {
            anyhow::bail!("el intervalo cron no puede ser cero");
        }
        if self.scheduler_started.swap(true, Ordering::AcqRel) {
            anyhow::bail!("el scheduler cron ya fue iniciado");
        }

        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = runtime.recover_and_dispatch().await {
                warn!(error = %error, "cron recovery failed");
            }
            let mut ticker = tokio::time::interval(tick_interval);
            loop {
                tokio::select! {
                    _ = runtime.shutdown.cancelled() => break,
                    _ = ticker.tick() => {
                        if let Err(error) = runtime.dispatch_due(Utc::now()).await {
                            warn!(error = %error, "cron scheduler tick failed");
                        }
                    }
                }
            }
        });
        Ok(())
    }

    pub fn stop(&self) {
        self.shutdown.cancel();
    }

    /// Claims and executes all jobs due at `now`. Exposed for hosts that want a
    /// deterministic tick and for integration tests; the background scheduler
    /// calls the same path.
    pub async fn dispatch_due(&self, now: DateTime<Utc>) -> Result<()> {
        self.store.sweep_expired(now).await?;
        let runs = self.store.claim_due(now, 100).await?;
        self.dispatch_runs(runs).await
    }

    async fn recover_and_dispatch(&self) -> Result<()> {
        self.store.sweep_expired(Utc::now()).await?;
        let runs = self.store.recover_interrupted_runs().await?;
        self.dispatch_runs(runs).await
    }

    async fn dispatch_runs(&self, runs: Vec<CronRun>) -> Result<()> {
        let results = join_all(runs.into_iter().map(|run| self.execute_run(run))).await;
        for result in results {
            result?;
        }
        Ok(())
    }

    async fn execute_run(&self, run: CronRun) -> Result<()> {
        let job =
            self.store.get_job(&run.job_id).await?.with_context(|| {
                format!("cron job {} no existe para run {}", run.job_id, run.id)
            })?;
        let lane = self
            .lanes
            .entry(run.target_thread_id.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _lane_guard = lane.lock().await;
        self.store.mark_run_running(&run.id, Utc::now()).await?;
        match self.executor.execute(job, run.clone()).await {
            Ok(CronRunOutcome::Succeeded(output)) => {
                self.store
                    .mark_run_succeeded(&run.id, Utc::now(), output.result)
                    .await?
            }
            Ok(CronRunOutcome::FailedNeedsUser { reason }) => {
                self.store
                    .mark_run_failed_needs_user(&run.id, Utc::now(), reason)
                    .await?
            }
            Err(error) => {
                self.store
                    .mark_run_failed(&run.id, Utc::now(), error.to_string())
                    .await?;
            }
        }
        Ok(())
    }
}
