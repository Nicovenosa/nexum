use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use chrono::{Duration as ChronoDuration, Utc};

use crate::transport::types::{CallerContext, HostPrincipal};
use nexum_agent::interaction::{
    ApprovalItem, InteractionContext, InteractionResponse, UserInteractionBroker,
};

use super::{
    ContinuationCapability, CronJobSpec, CronRunExecutor, CronRunOutcome, CronRunOutput,
    CronRunStatus, CronRuntime, HeadlessFailSafeBroker, InteractionPolicy,
    OwnerPrincipalAuthorizer, PendingInteractionBroker, PendingInteractionSink,
    PendingInteractionSpec, PendingInteractionStatus, ResolvePendingInteractionRequest,
    ResolvePendingInteractionStatus, SqliteCronStore, SqlitePendingInteractionBroker,
};

struct MockExecutor {
    active: AtomicUsize,
    peak_active: AtomicUsize,
}

#[derive(Default)]
struct MockPendingInteractionSink {
    recorded: tokio::sync::Mutex<Vec<PendingInteractionSpec>>,
}

#[async_trait::async_trait]
impl PendingInteractionSink for MockPendingInteractionSink {
    async fn persist_pending(&self, interaction: PendingInteractionSpec) -> anyhow::Result<()> {
        self.recorded.lock().await.push(interaction);
        Ok(())
    }
}

#[async_trait::async_trait]
impl CronRunExecutor for MockExecutor {
    async fn execute(
        &self,
        _job: super::CronJob,
        _run: super::CronRun,
    ) -> anyhow::Result<CronRunOutcome> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_active.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(25)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(CronRunOutcome::Succeeded(CronRunOutput::text(
            "ejecución completada",
        )))
    }
}

struct MockNeedsUserExecutor;

#[async_trait::async_trait]
impl CronRunExecutor for MockNeedsUserExecutor {
    async fn execute(
        &self,
        _job: super::CronJob,
        _run: super::CronRun,
    ) -> anyhow::Result<CronRunOutcome> {
        Ok(CronRunOutcome::FailedNeedsUser {
            reason: "la operación requiere autorización del usuario".to_string(),
        })
    }
}

async fn make_store() -> SqliteCronStore {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.keep().join("cron.db");
    SqliteCronStore::open(path).await.unwrap()
}

fn make_job(target_thread_id: &str) -> CronJobSpec {
    CronJobSpec {
        target_thread_id: target_thread_id.to_string(),
        owner_principal: make_principal("test-owner"),
        schedule: "* * * * *".to_string(),
        prompt: "revisá el estado del proyecto".to_string(),
    }
}

fn make_principal(value: &str) -> HostPrincipal {
    HostPrincipal::new(value).unwrap()
}

fn make_caller(connection_id: u64, principal: &HostPrincipal) -> CallerContext {
    CallerContext::from_connection(connection_id, Some(principal.clone()))
}

#[tokio::test]
async fn test_store_persiste_job_durable_y_migra_schema() {
    let store = make_store().await;

    let created = store.create_job(make_job("thread-a")).await.unwrap();
    let listed = store.list_jobs().await.unwrap();

    assert_eq!(listed, vec![created]);
    assert_eq!(listed[0].target_thread_id, "thread-a");
    assert_eq!(
        listed[0].owner_principal,
        Some(make_principal("test-owner"))
    );
}

#[tokio::test]
async fn test_store_reclama_un_vencimiento_una_sola_vez_y_guarda_run() {
    let store = make_store().await;
    let job = store.create_job(make_job("thread-a")).await.unwrap();
    let due_at = job.next_run_at + ChronoDuration::seconds(1);

    let first_claim = store.claim_due(due_at, 10).await.unwrap();
    let second_claim = store.claim_due(due_at, 10).await.unwrap();

    assert_eq!(first_claim.len(), 1);
    assert!(second_claim.is_empty());
    assert_eq!(first_claim[0].job_id, job.id);
    assert_eq!(first_claim[0].status, CronRunStatus::Queued);
}

#[tokio::test]
async fn test_store_recupera_run_interrumpido_como_queued() {
    let store = make_store().await;
    store.create_job(make_job("thread-a")).await.unwrap();
    let due_at = Utc::now() + ChronoDuration::minutes(2);
    let run = store.claim_due(due_at, 10).await.unwrap().remove(0);
    store.mark_run_running(&run.id, Utc::now()).await.unwrap();

    let recovered = store.recover_interrupted_runs().await.unwrap();

    assert_eq!(recovered, vec![run.clone()]);
    assert_eq!(
        store.get_run(&run.id).await.unwrap().unwrap().status,
        CronRunStatus::Queued
    );
}

#[tokio::test]
async fn test_store_no_reencola_run_interrumpido_que_ya_tiene_interaccion_durable() {
    let store = make_store().await;
    store.create_job(make_job("thread-a")).await.unwrap();
    let run = store
        .claim_due(Utc::now() + ChronoDuration::minutes(2), 10)
        .await
        .unwrap()
        .remove(0);
    store.mark_run_running(&run.id, Utc::now()).await.unwrap();
    store
        .create_pending_interaction(PendingInteractionSpec {
            run_id: run.id.clone(),
            job_id: run.job_id.clone(),
            target_thread_id: run.target_thread_id.clone(),
            context: InteractionContext::Approval { items: Vec::new() },
            expires_at: Utc::now() + ChronoDuration::hours(1),
        })
        .await
        .unwrap();

    let recovered = store.recover_interrupted_runs().await.unwrap();

    assert!(recovered.is_empty());
    assert_eq!(
        store.get_run(&run.id).await.unwrap().unwrap().status,
        CronRunStatus::FailedNeedsUser
    );
}

#[tokio::test]
async fn test_store_persiste_interaccion_pendiente_y_expira_sin_continuacion() {
    let store = make_store().await;
    store.create_job(make_job("thread-a")).await.unwrap();
    let run = store
        .claim_due(Utc::now() + ChronoDuration::minutes(2), 10)
        .await
        .unwrap()
        .remove(0);
    let expires_at = Utc::now() + ChronoDuration::minutes(1);

    let pending = store
        .create_pending_interaction(PendingInteractionSpec {
            run_id: run.id.clone(),
            job_id: run.job_id.clone(),
            target_thread_id: run.target_thread_id.clone(),
            context: InteractionContext::Approval {
                items: vec![ApprovalItem {
                    tool_call_id: "call-1".to_string(),
                    tool_name: "Bash".to_string(),
                    tool_input: serde_json::json!({"command": "rm -rf /"}),
                }],
            },
            expires_at,
        })
        .await
        .unwrap();

    assert_eq!(pending.status, PendingInteractionStatus::Pending);
    assert_eq!(
        pending.continuation_capability,
        ContinuationCapability::Unsupported
    );
    assert_eq!(pending.run_id, run.id);
    assert_eq!(
        store
            .sweep_expired(expires_at + ChronoDuration::seconds(1))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .get_pending_interaction(&pending.id, "thread-a")
            .await
            .unwrap()
            .unwrap()
            .status,
        PendingInteractionStatus::Expired
    );
}

#[tokio::test]
async fn test_runtime_serializa_runs_del_mismo_target_thread() {
    let store = Arc::new(make_store().await);
    store.create_job(make_job("thread-a")).await.unwrap();
    store.create_job(make_job("thread-a")).await.unwrap();
    let executor = Arc::new(MockExecutor {
        active: AtomicUsize::new(0),
        peak_active: AtomicUsize::new(0),
    });
    let runtime = CronRuntime::new(store.clone(), executor.clone());

    let due_at = store.list_jobs().await.unwrap()[0].next_run_at + ChronoDuration::seconds(1);
    runtime.dispatch_due(due_at).await.unwrap();

    assert_eq!(executor.peak_active.load(Ordering::SeqCst), 1);
    assert!(store
        .list_runs()
        .await
        .unwrap()
        .iter()
        .all(|run| run.status == CronRunStatus::Succeeded));
}

#[tokio::test]
async fn test_runtime_persiste_resultado_del_run_exitoso() {
    let store = Arc::new(make_store().await);
    let job = store.create_job(make_job("thread-a")).await.unwrap();
    let executor = Arc::new(MockExecutor {
        active: AtomicUsize::new(0),
        peak_active: AtomicUsize::new(0),
    });
    let runtime = CronRuntime::new(store.clone(), executor);

    runtime
        .dispatch_due(job.next_run_at + ChronoDuration::seconds(1))
        .await
        .unwrap();

    let run = store.list_runs().await.unwrap().remove(0);
    assert_eq!(run.status, CronRunStatus::Succeeded);
    assert_eq!(run.result.as_deref(), Some("ejecución completada"));
}

#[tokio::test]
async fn test_runtime_finaliza_run_necesita_usuario_sin_reintentar() {
    let store = Arc::new(make_store().await);
    let job = store.create_job(make_job("thread-a")).await.unwrap();
    let runtime = CronRuntime::new(store.clone(), Arc::new(MockNeedsUserExecutor));

    runtime
        .dispatch_due(job.next_run_at + ChronoDuration::seconds(1))
        .await
        .unwrap();

    let run = store.list_runs().await.unwrap().remove(0);
    assert_eq!(run.status, CronRunStatus::FailedNeedsUser);
    assert!(run.finished_at.is_some());
    assert!(run
        .error
        .as_deref()
        .unwrap()
        .contains("requiere autorización"));
    assert!(store.queued_runs().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_broker_rechaza_resolver_sin_autorizador() {
    let store = Arc::new(make_store().await);
    store.create_job(make_job("thread-a")).await.unwrap();
    let run = store
        .claim_due(Utc::now() + ChronoDuration::minutes(2), 10)
        .await
        .unwrap()
        .remove(0);
    store.mark_run_running(&run.id, Utc::now()).await.unwrap();
    store
        .mark_run_failed_needs_user(&run.id, Utc::now(), "autorización requerida".to_string())
        .await
        .unwrap();
    let pending = store
        .create_pending_interaction(PendingInteractionSpec {
            run_id: run.id.clone(),
            job_id: run.job_id.clone(),
            target_thread_id: run.target_thread_id.clone(),
            context: InteractionContext::Approval { items: Vec::new() },
            expires_at: Utc::now() + ChronoDuration::hours(1),
        })
        .await
        .unwrap();
    let broker = SqlitePendingInteractionBroker::new(store.clone());

    let result = broker
        .resolve_pending_interaction(ResolvePendingInteractionRequest {
            interaction_id: pending.id,
            target_thread_id: "thread-a".to_string(),
            caller: make_caller(1, &make_principal("test-owner")),
            status: ResolvePendingInteractionStatus::Approved,
            note: None,
        })
        .await;

    assert!(
        result.is_err(),
        "sin autorizador no se puede resolver una interacción"
    );
    assert_eq!(
        store.get_run(&run.id).await.unwrap().unwrap().status,
        CronRunStatus::FailedNeedsUser
    );
    assert!(store.queued_runs().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_broker_reconexion_del_owner_autoriza_y_otro_principal_no() {
    let store = Arc::new(make_store().await);
    let owner = make_principal("owner-a");
    let mut job = make_job("thread-a");
    job.owner_principal = owner.clone();
    store.create_job(job).await.unwrap();
    let run = store
        .claim_due(Utc::now() + ChronoDuration::minutes(2), 10)
        .await
        .unwrap()
        .remove(0);
    store.mark_run_running(&run.id, Utc::now()).await.unwrap();
    store
        .mark_run_failed_needs_user(&run.id, Utc::now(), "autorización requerida".to_string())
        .await
        .unwrap();
    let pending = store
        .create_pending_interaction(PendingInteractionSpec {
            run_id: run.id.clone(),
            job_id: run.job_id.clone(),
            target_thread_id: run.target_thread_id.clone(),
            context: InteractionContext::Approval { items: Vec::new() },
            expires_at: Utc::now() + ChronoDuration::hours(1),
        })
        .await
        .unwrap();
    let broker = SqlitePendingInteractionBroker::new(store.clone())
        .with_authorizer(Arc::new(OwnerPrincipalAuthorizer));

    let other_principal = make_principal("owner-b");
    let rejected = broker
        .resolve_pending_interaction(ResolvePendingInteractionRequest {
            interaction_id: pending.id.clone(),
            target_thread_id: "thread-a".to_string(),
            caller: make_caller(3, &other_principal),
            status: ResolvePendingInteractionStatus::Approved,
            note: None,
        })
        .await;

    assert!(
        rejected.is_err(),
        "otro principal no puede resolver el pending"
    );

    let resolved = broker
        .resolve_pending_interaction(ResolvePendingInteractionRequest {
            interaction_id: pending.id,
            target_thread_id: "thread-a".to_string(),
            caller: make_caller(2, &owner),
            status: ResolvePendingInteractionStatus::Approved,
            note: None,
        })
        .await
        .unwrap();

    assert_eq!(resolved.status, PendingInteractionStatus::Approved);
    assert_eq!(resolved.owner_principal.as_ref(), Some(&owner));
    assert_eq!(
        store.get_run(&run.id).await.unwrap().unwrap().status,
        CronRunStatus::FailedNeedsUser
    );
    assert!(store.queued_runs().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_broker_headless_rechaza_aprobaciones_sin_cliente() {
    let store = Arc::new(make_store().await);
    let job = store.create_job(make_job("thread-a")).await.unwrap();
    let run = store
        .claim_due(job.next_run_at + ChronoDuration::seconds(1), 10)
        .await
        .unwrap()
        .remove(0);
    let sink = Arc::new(MockPendingInteractionSink::default());
    let cancel = nexum_agent::agent::AgentCancellationToken::new();
    let broker = HeadlessFailSafeBroker::new(
        InteractionPolicy::FailSafely,
        sink.clone(),
        job,
        run.clone(),
        cancel.clone(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );

    let response = broker
        .request(InteractionContext::Approval {
            items: vec![ApprovalItem {
                tool_call_id: "call-1".to_string(),
                tool_name: "Bash".to_string(),
                tool_input: serde_json::json!({"command": "rm -rf /"}),
            }],
        })
        .await;

    let InteractionResponse::Decisions(decisions) = response else {
        panic!("el broker headless debe devolver decisiones de aprobación");
    };
    assert!(matches!(
        decisions.as_slice(),
        [nexum_agent::interaction::ApprovalDecision::Reject { reason, .. }]
            if reason.contains("sin cliente")
    ));
    assert!(cancel.is_cancelled());
    let recorded = sink.recorded.lock().await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].run_id, run.id);
    assert_eq!(recorded[0].target_thread_id, "thread-a");
}

#[tokio::test]
async fn test_runtime_rechaza_iniciar_un_segundo_scheduler() {
    let store = Arc::new(make_store().await);
    let executor = Arc::new(MockExecutor {
        active: AtomicUsize::new(0),
        peak_active: AtomicUsize::new(0),
    });
    let runtime = Arc::new(CronRuntime::new(store, executor));

    runtime.start(Duration::from_secs(60)).unwrap();
    let second_start = runtime.start(Duration::from_secs(60));
    runtime.stop();

    assert!(second_start.is_err());
}

#[test]
fn test_capabilities_publicitan_interacciones_durables_sin_continuacion() {
    let capabilities = CronRuntime::capabilities();

    assert!(capabilities.execute_prompt_adapter);
    assert!(capabilities.headless_execution);
    assert!(capabilities.requires_host_prompt_context);
    assert!(capabilities.durable_pending_interactions);
    assert!(capabilities.fail_safely_interaction_policy);
    assert!(!capabilities.interaction_continuation_supported);
    assert!(capabilities.acp_management_api);
}
