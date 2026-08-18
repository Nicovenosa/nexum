use std::sync::Arc;

use crate::cron::{CronControlClient, CronControlError, CronControlPort, CronTask};
use async_trait::async_trait;
use parking_lot::Mutex;

struct MockCronControl {
    tasks: Mutex<Vec<CronTask>>,
}

#[async_trait]
impl CronControlPort for MockCronControl {
    async fn register(&self, expression: &str, prompt: &str) -> Result<String, CronControlError> {
        let id = format!("mock-{}", self.tasks.lock().len() + 1);
        self.tasks.lock().push(CronTask {
            id: id.clone(),
            expression: expression.to_string(),
            prompt: prompt.to_string(),
            next_fire: None,
            enabled: true,
        });
        Ok(id)
    }

    async fn list(&self) -> Result<Vec<CronTask>, CronControlError> {
        Ok(self.tasks.lock().clone())
    }

    async fn remove(&self, id: &str) -> Result<(), CronControlError> {
        let mut tasks = self.tasks.lock();
        let before = tasks.len();
        tasks.retain(|task| task.id != id);
        if tasks.len() == before {
            return Err(CronControlError::Failed("task not found".to_string()));
        }
        Ok(())
    }
}

fn new_tools() -> (CronRegisterTool, CronListTool, CronRemoveTool) {
    let client = CronControlClient::new(Arc::new(MockCronControl {
        tasks: Mutex::new(Vec::new()),
    }));
    (
        CronRegisterTool::new(client.clone()),
        CronListTool::new(client.clone()),
        CronRemoveTool::new(client),
    )
}

#[tokio::test]
async fn test_register_rejects_empty_prompt() {
    let (reg, _, _) = new_tools();
    let result = reg
        .invoke(
            serde_json::json!({"expression": "* * * * *", "prompt": ""}),
            nexum_agent::tools::ToolContext::new(&[], "."),
        )
        .await;
    assert!(result.is_err(), "空 prompt 应被拒绝");
}

#[tokio::test]
async fn test_register_reports_cron_unavailable_without_host() {
    let tool = CronRegisterTool::new(CronControlClient::unavailable());
    let result = tool
        .invoke(
            serde_json::json!({"expression": "* * * * *", "prompt": "host required"}),
            nexum_agent::tools::ToolContext::new(&[], "."),
        )
        .await;
    let error = result.expect_err("a missing host must not create a local cron scheduler");
    assert!(error.to_string().contains("CronUnavailable"));
}

#[tokio::test]
async fn test_register_rejects_whitespace_prompt() {
    let (reg, _, _) = new_tools();
    let result = reg
        .invoke(
            serde_json::json!({"expression": "* * * * *", "prompt": "   "}),
            nexum_agent::tools::ToolContext::new(&[], "."),
        )
        .await;
    assert!(result.is_err(), "纯空白 prompt 应被拒绝");
}

#[tokio::test]
async fn test_register_success() {
    let (reg, list, _) = new_tools();
    let result = reg
        .invoke(
            serde_json::json!({"expression": "* * * * *", "prompt": "test task"}),
            nexum_agent::tools::ToolContext::new(&[], "."),
        )
        .await
        .unwrap();
    assert!(result.contains("已注册"));

    let list_result = list
        .invoke(
            serde_json::json!({}),
            nexum_agent::tools::ToolContext::new(&[], "."),
        )
        .await
        .unwrap();
    assert!(list_result.contains("test task"));
}
