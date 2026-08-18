use async_trait::async_trait;
use nexum_agent::{agent::state::State, middleware::r#trait::Middleware, tools::BaseTool};

use super::{
    tools::{CronListTool, CronRegisterTool, CronRemoveTool},
    CronControlClient,
};

/// Cron 中间件：提供 cron_register / cron_list / cron_remove 工具
pub struct CronMiddleware {
    client: CronControlClient,
}

impl CronMiddleware {
    pub fn new(client: CronControlClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl<S: State> Middleware<S> for CronMiddleware {
    fn name(&self) -> &str {
        "CronMiddleware"
    }

    fn collect_tools(&self, _cwd: &str) -> Vec<Box<dyn BaseTool>> {
        let client = self.client.clone();
        vec![
            Box::new(CronRegisterTool::new(client.clone())),
            Box::new(CronListTool::new(client.clone())),
            Box::new(CronRemoveTool::new(client)),
        ]
    }
}
