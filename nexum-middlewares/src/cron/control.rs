use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use super::CronTask;

/// Error reported when a frontend has no connection to a host-owned cron runtime.
#[derive(Debug, Error)]
pub enum CronControlError {
    #[error("CronUnavailable: {0}")]
    Unavailable(&'static str),
    #[error("cron control failed: {0}")]
    Failed(String),
}

/// Minimal control boundary for a host-owned cron runtime.
///
/// The protocol adapter owns identity, authorization and transport. Frontends
/// only use this contract; they never construct a scheduler as a fallback.
#[async_trait]
pub trait CronControlPort: Send + Sync {
    async fn register(&self, expression: &str, prompt: &str) -> Result<String, CronControlError>;
    async fn list(&self) -> Result<Vec<CronTask>, CronControlError>;
    async fn remove(&self, id: &str) -> Result<(), CronControlError>;
}

#[derive(Clone)]
pub struct CronControlClient {
    port: Arc<dyn CronControlPort>,
}

impl CronControlClient {
    pub fn new(port: Arc<dyn CronControlPort>) -> Self {
        Self { port }
    }

    pub fn unavailable() -> Self {
        Self::new(Arc::new(CronUnavailablePort))
    }

    pub async fn register(
        &self,
        expression: &str,
        prompt: &str,
    ) -> Result<String, CronControlError> {
        self.port.register(expression, prompt).await
    }

    pub async fn list(&self) -> Result<Vec<CronTask>, CronControlError> {
        self.port.list().await
    }

    pub async fn remove(&self, id: &str) -> Result<(), CronControlError> {
        self.port.remove(id).await
    }
}

struct CronUnavailablePort;

#[async_trait]
impl CronControlPort for CronUnavailablePort {
    async fn register(&self, _expression: &str, _prompt: &str) -> Result<String, CronControlError> {
        Err(CronControlError::Unavailable("no cron host is connected"))
    }

    async fn list(&self) -> Result<Vec<CronTask>, CronControlError> {
        Err(CronControlError::Unavailable("no cron host is connected"))
    }

    async fn remove(&self, _id: &str) -> Result<(), CronControlError> {
        Err(CronControlError::Unavailable("no cron host is connected"))
    }
}
