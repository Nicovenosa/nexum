use std::sync::Arc;

use crate::transport::types::CallerContext;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use super::{
    PendingInteraction, PendingInteractionSpec, PendingInteractionStatus, SqliteCronStore,
};

/// Headless cron execution must never wait for a user or guess a decision.
/// The durable record remains visible for audit, but cannot resume its agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionPolicy {
    FailSafely,
}

impl InteractionPolicy {
    pub(crate) fn expires_at(self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Self::FailSafely => now + Duration::hours(24),
        }
    }
}

/// Host-neutral persistence boundary for interactions observed by headless
/// execution. Middleware only speaks to `UserInteractionBroker`; it never
/// reaches SQLite or any host persistence implementation directly.
#[async_trait]
pub trait PendingInteractionSink: Send + Sync {
    async fn persist_pending(&self, interaction: PendingInteractionSpec) -> Result<()>;
}

#[async_trait]
impl PendingInteractionSink for SqliteCronStore {
    async fn persist_pending(&self, interaction: PendingInteractionSpec) -> Result<()> {
        self.create_pending_interaction(interaction)
            .await
            .map(|_| ())
    }
}

/// The action a host authorizer can restrict when it has a user identity model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingInteractionAction {
    Read,
    Resolve,
}

/// Host-owned authorization hook for durable pending interactions.
#[async_trait]
pub trait PendingInteractionAuthorizer: Send + Sync {
    async fn authorize(
        &self,
        caller: &CallerContext,
        action: PendingInteractionAction,
        interaction: &PendingInteraction,
    ) -> Result<()>;
}

/// Authorizes only the durable owner captured with a pending interaction.
/// Hosts authenticate the principal; this policy does not use connection IDs.
pub struct OwnerPrincipalAuthorizer;

#[async_trait]
impl PendingInteractionAuthorizer for OwnerPrincipalAuthorizer {
    async fn authorize(
        &self,
        caller: &CallerContext,
        _action: PendingInteractionAction,
        interaction: &PendingInteraction,
    ) -> Result<()> {
        let principal = caller
            .principal()
            .ok_or_else(|| anyhow::anyhow!("el caller no tiene principal autenticado"))?;
        let owner = interaction
            .owner_principal
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("la interacción no tiene owner durable"))?;
        if principal == owner {
            Ok(())
        } else {
            anyhow::bail!("el principal no está autorizado para la interacción")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPendingInteractionsRequest {
    pub target_thread_id: Option<String>,
    pub caller: CallerContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetPendingInteractionRequest {
    pub interaction_id: String,
    pub target_thread_id: String,
    pub caller: CallerContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvePendingInteractionStatus {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvePendingInteractionRequest {
    pub interaction_id: String,
    pub target_thread_id: String,
    pub caller: CallerContext,
    pub status: ResolvePendingInteractionStatus,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingInteractionCapabilities {
    pub durable_pending_interactions: bool,
    pub continuation_supported: bool,
    pub authorization_enforced: bool,
}

/// Host-neutral management boundary used by ACP. Approving a record changes
/// audit state only; implementations must never use it to restart execution.
#[async_trait]
pub trait PendingInteractionBroker: Send + Sync {
    fn capabilities(&self) -> PendingInteractionCapabilities;
    async fn list_pending_interactions(
        &self,
        request: ListPendingInteractionsRequest,
    ) -> Result<Vec<PendingInteraction>>;
    async fn get_pending_interaction(
        &self,
        request: GetPendingInteractionRequest,
    ) -> Result<PendingInteraction>;
    async fn resolve_pending_interaction(
        &self,
        request: ResolvePendingInteractionRequest,
    ) -> Result<PendingInteraction>;
}

/// SQLite implementation selected by the local host. ACP and middleware use
/// only the contracts above, so another host may provide a different backend.
pub struct SqlitePendingInteractionBroker {
    store: Arc<SqliteCronStore>,
    authorizer: Option<Arc<dyn PendingInteractionAuthorizer>>,
}

impl SqlitePendingInteractionBroker {
    pub fn new(store: Arc<SqliteCronStore>) -> Self {
        Self {
            store,
            authorizer: None,
        }
    }

    pub fn with_authorizer(mut self, authorizer: Arc<dyn PendingInteractionAuthorizer>) -> Self {
        self.authorizer = Some(authorizer);
        self
    }

    async fn authorize(
        &self,
        caller: &CallerContext,
        action: PendingInteractionAction,
        interaction: &PendingInteraction,
    ) -> Result<()> {
        let authorizer = self
            .authorizer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("la interacción requiere un autorizador del host"))?;
        authorizer.authorize(caller, action, interaction).await
    }

    fn validate_id(value: &str, name: &str) -> Result<()> {
        if value.trim().is_empty() {
            anyhow::bail!("{name} no puede estar vacío");
        }
        Ok(())
    }
}

#[async_trait]
impl PendingInteractionBroker for SqlitePendingInteractionBroker {
    fn capabilities(&self) -> PendingInteractionCapabilities {
        PendingInteractionCapabilities {
            durable_pending_interactions: true,
            continuation_supported: false,
            authorization_enforced: self.authorizer.is_some(),
        }
    }

    async fn list_pending_interactions(
        &self,
        request: ListPendingInteractionsRequest,
    ) -> Result<Vec<PendingInteraction>> {
        if let Some(target_thread_id) = &request.target_thread_id {
            Self::validate_id(target_thread_id, "target_thread_id")?;
        }
        self.store.sweep_expired(Utc::now()).await?;
        let interactions = self
            .store
            .list_pending_interactions(request.target_thread_id.as_deref())
            .await?;
        for interaction in &interactions {
            self.authorize(&request.caller, PendingInteractionAction::Read, interaction)
                .await?;
        }
        Ok(interactions)
    }

    async fn get_pending_interaction(
        &self,
        request: GetPendingInteractionRequest,
    ) -> Result<PendingInteraction> {
        Self::validate_id(&request.interaction_id, "interaction_id")?;
        Self::validate_id(&request.target_thread_id, "target_thread_id")?;
        self.store.sweep_expired(Utc::now()).await?;
        let interaction = self
            .store
            .get_pending_interaction(&request.interaction_id, &request.target_thread_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("interacción no encontrada para el target indicado"))?;
        self.authorize(
            &request.caller,
            PendingInteractionAction::Read,
            &interaction,
        )
        .await?;
        Ok(interaction)
    }

    async fn resolve_pending_interaction(
        &self,
        request: ResolvePendingInteractionRequest,
    ) -> Result<PendingInteraction> {
        let interaction = self
            .get_pending_interaction(GetPendingInteractionRequest {
                interaction_id: request.interaction_id.clone(),
                target_thread_id: request.target_thread_id.clone(),
                caller: request.caller.clone(),
            })
            .await?;
        if interaction.status != PendingInteractionStatus::Pending {
            anyhow::bail!("la interacción ya no está pendiente");
        }
        self.authorize(
            &request.caller,
            PendingInteractionAction::Resolve,
            &interaction,
        )
        .await?;
        let status = match request.status {
            ResolvePendingInteractionStatus::Approved => PendingInteractionStatus::Approved,
            ResolvePendingInteractionStatus::Rejected => PendingInteractionStatus::Rejected,
        };
        self.store
            .resolve_pending_interaction(
                &request.interaction_id,
                &request.target_thread_id,
                status,
                request.note,
                Utc::now(),
            )
            .await
    }
}
