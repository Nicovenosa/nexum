//! Durable, host-neutral scheduling primitives for ACP hosts.
//!
//! The runtime owns scheduling, persistence and per-thread serialization. A host
//! supplies the execution adapter; [`ExecutePromptRunner`] delegates execution to
//! the existing ACP [`crate::session::executor::execute_prompt`] pipeline rather
//! than creating another agent loop, provider or thread store.

mod context;
mod executor;
mod interaction;
mod model;
mod runtime;
mod store;

pub use context::{CronPromptResources, HeadlessPromptContextFactory};
pub use executor::{
    CronPromptContextFactory, CronPromptExecutionContext, CronRunExecutor, CronRunOutcome,
    CronRunOutput, ExecutePromptRunner, HeadlessFailSafeBroker,
};
pub use interaction::{
    GetPendingInteractionRequest, InteractionPolicy, ListPendingInteractionsRequest,
    OwnerPrincipalAuthorizer, PendingInteractionAction, PendingInteractionAuthorizer,
    PendingInteractionBroker, PendingInteractionCapabilities, PendingInteractionSink,
    ResolvePendingInteractionRequest, ResolvePendingInteractionStatus,
    SqlitePendingInteractionBroker,
};
pub use model::{
    ContinuationCapability, CronJob, CronJobSpec, CronRun, CronRunStatus, PendingInteraction,
    PendingInteractionSpec, PendingInteractionStatus,
};
pub use nexum_middlewares::cron::{CronControlClient, CronControlError, CronControlPort};
pub use runtime::{CronCapabilities, CronRuntime};
pub use store::SqliteCronStore;

#[cfg(test)]
mod mod_test;
