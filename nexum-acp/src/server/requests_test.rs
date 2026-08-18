use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use nexum_acp::provider::{NexumConfig, ProviderConfig, ProviderModels};
use nexum_acp::transport::types::{AcpError, CallerContext, IncomingMessage, RequestId};
use nexum_agent::thread::FilesystemThreadStore;
use nexum_middlewares::hitl::shared_mode::{PermissionMode, SharedPermissionMode};
use serde_json::{json, Value};

use super::*;
use crate::{
    provider::LlmProvider,
    runtime::{
        CapabilityState, RuntimeCapabilities, RuntimeHealth, RuntimeHealthState, RuntimeIdentity,
        RuntimeIdentityInput, RuntimeMetadata,
    },
    server::NoopPredictionPolicy,
};

// ── Mock AcpTransport ─────────────────────────────────────────────────────────

/// 丢弃所有发送操作的 mock transport
struct MockTransport;

#[test]
fn test_session_response_expone_thread_id_sin_forzar_igualdad_con_session_id() {
    let response = session_response_with_thread(json!({"sessionId": "session-a"}), "thread-a")
        .unwrap();
    assert_eq!(response["sessionId"], "session-a");
    assert_eq!(response["threadId"], "thread-a");
    assert_ne!(response["sessionId"], response["threadId"]);
}

struct MockPendingInteractionBroker {
    authorized_caller: CallerContext,
}

impl MockPendingInteractionBroker {
    fn new(connection_id: u64) -> Self {
        Self {
            authorized_caller: CallerContext::from_connection(connection_id, None),
        }
    }
}

fn make_pending_interaction(
    status: nexum_acp::cron::PendingInteractionStatus,
) -> nexum_acp::cron::PendingInteraction {
    let now = Utc::now();
    nexum_acp::cron::PendingInteraction {
        id: "interaction-1".to_string(),
        run_id: "run-1".to_string(),
        job_id: "job-1".to_string(),
        target_thread_id: "thread-a".to_string(),
        owner_principal: None,
        context: nexum_agent::interaction::InteractionContext::Approval { items: Vec::new() },
        status,
        continuation_capability: nexum_acp::cron::ContinuationCapability::Unsupported,
        created_at: now,
        expires_at: now + chrono::Duration::hours(1),
        resolved_at: None,
        resolution_note: None,
    }
}

#[async_trait]
impl nexum_acp::cron::PendingInteractionBroker for MockPendingInteractionBroker {
    fn capabilities(&self) -> nexum_acp::cron::PendingInteractionCapabilities {
        nexum_acp::cron::PendingInteractionCapabilities {
            durable_pending_interactions: true,
            continuation_supported: false,
            authorization_enforced: true,
        }
    }

    async fn list_pending_interactions(
        &self,
        _request: nexum_acp::cron::ListPendingInteractionsRequest,
    ) -> anyhow::Result<Vec<nexum_acp::cron::PendingInteraction>> {
        Ok(vec![make_pending_interaction(
            nexum_acp::cron::PendingInteractionStatus::Pending,
        )])
    }

    async fn get_pending_interaction(
        &self,
        _request: nexum_acp::cron::GetPendingInteractionRequest,
    ) -> anyhow::Result<nexum_acp::cron::PendingInteraction> {
        Ok(make_pending_interaction(
            nexum_acp::cron::PendingInteractionStatus::Pending,
        ))
    }

    async fn resolve_pending_interaction(
        &self,
        request: nexum_acp::cron::ResolvePendingInteractionRequest,
    ) -> anyhow::Result<nexum_acp::cron::PendingInteraction> {
        if request.caller != self.authorized_caller {
            anyhow::bail!("caller no autorizado")
        }
        Ok(make_pending_interaction(
            nexum_acp::cron::PendingInteractionStatus::Approved,
        ))
    }
}

#[async_trait]
impl nexum_acp::transport::AcpTransport for MockTransport {
    async fn send_request(&self, _method: &str, _params: Value) -> Result<Value, AcpError> {
        Ok(json!({}))
    }
    async fn send_notification(&self, _method: &str, _params: Value) -> Result<(), AcpError> {
        Ok(())
    }
    async fn recv(&self) -> Option<IncomingMessage> {
        None
    }
    async fn send_response(
        &self,
        _id: RequestId,
        _result: Result<Value, AcpError>,
    ) -> Result<(), AcpError> {
        Ok(())
    }
}

// ── 辅助函数 ──────────────────────────────────────────────────────────────────

fn make_provider_config(
    id: &str,
    provider_type: &str,
    api_key: &str,
    model: &str,
) -> ProviderConfig {
    ProviderConfig {
        id: id.to_string(),
        provider_type: provider_type.to_string(),
        api_key: api_key.to_string(),
        // 将模型名填入 sonnet 别名（默认 alias）
        models: ProviderModels {
            sonnet: model.to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn make_server_config(
    nexum_config: NexumConfig,
    provider: LlmProvider,
    tmp: &tempfile::TempDir,
) -> AcpServerConfig {
    let thread_store = FilesystemThreadStore::new(tmp.path().join("threads"));
    let arc_thread_store: Arc<dyn nexum_agent::thread::ThreadStore> = Arc::new(thread_store);
    let session_manager = nexum_acp::session::SessionManager::new(
        arc_thread_store.clone(),
        provider.clone(),
        Arc::new(nexum_config.clone()),
        SharedPermissionMode::new(PermissionMode::Bypass),
        None,
    );
    AcpServerConfig {
        provider: Arc::new(parking_lot::RwLock::new(provider)),
        nexum_config: Arc::new(parking_lot::RwLock::new(nexum_config)),
        permission_mode: SharedPermissionMode::new(PermissionMode::Bypass),
        cron_control: nexum_middlewares::cron::CronControlClient::unavailable(),
        mcp_pool: None,
        channel_state: None,
        plugin_skill_roots: Vec::new(),
        plugin_agent_dirs: Vec::new(),
        plugin_hooks: Vec::new(),
        hook_groups: Vec::new(),
        plugin_lsp_servers: Vec::new(),
        tool_search_index: Arc::new(nexum_middlewares::tool_search::ToolSearchIndex::new()),
        shared_tools: Arc::new(parking_lot::RwLock::new(HashMap::new())),
        thread_store: arc_thread_store,
        langfuse_session: None,
        config_path: tmp.path().join("test_config.json"),
        session_manager,
        prediction_policy: Arc::new(NoopPredictionPolicy),
        pending_interaction_broker: None,
        runtime: RuntimeMetadata {
            identity: Arc::new(parking_lot::RwLock::new(RuntimeIdentity::new(
                RuntimeIdentityInput::default(),
            ))),
            capabilities: RuntimeCapabilities::new(
                "nexum.acp.capabilities/v1",
                [(
                    "cron",
                    CapabilityState::Unavailable {
                        reason: "no cron host is connected".to_string(),
                    },
                )],
                std::iter::empty::<(&str, Vec<String>)>(),
            ),
            health: RuntimeHealth::new(RuntimeHealthState::Ready),
        },
    }
}

#[tokio::test]
async fn test_runtime_rpc_expone_identidad_capacidades_y_health() {
    let tmp = tempfile::TempDir::new().unwrap();
    let provider_config = make_provider_config("test", "openai", "test-key", "test-model");
    let mut nexum_config = NexumConfig::default();
    nexum_config.config.active_provider_id = "test".to_string();
    nexum_config.config.providers = vec![provider_config];
    let provider = LlmProvider::from_config(&nexum_config).unwrap();
    let cfg = make_server_config(nexum_config, provider, &tmp);
    let mut sessions = HashMap::new();
    let transport = MockTransport;

    let identity = handle_request(
        "runtime/identity",
        &json!({}),
        None,
        &cfg,
        &mut sessions,
        &transport,
    )
    .await
    .unwrap();
    let capabilities = handle_request(
        "runtime/capabilities",
        &json!({}),
        None,
        &cfg,
        &mut sessions,
        &transport,
    )
    .await
    .unwrap();

    assert_eq!(identity["protocol_schema"], "nexum.acp.runtime/v1");
    assert!(identity["runtime_instance_id"].as_str().is_some());
    assert_eq!(capabilities["health"], "ready");
    assert_eq!(
        capabilities["capabilities"]["resources"]["cron"]["state"],
        "unavailable"
    );
    assert!(capabilities["capabilities"]["hash"].as_str().is_some());
}

#[tokio::test]
async fn test_set_config_option_actualiza_identidad_runtime_con_provider_y_modelo_actuales() {
    let tmp = tempfile::TempDir::new().unwrap();
    let provider_config = make_provider_config("test", "openai", "test-key", "test-model");
    let mut nexum_config = NexumConfig::default();
    nexum_config.config.active_provider_id = "test".to_string();
    nexum_config.config.active_alias = "test-model".to_string();
    nexum_config.config.providers = vec![provider_config];
    let provider = LlmProvider::from_config(&nexum_config).unwrap();
    let cfg = make_server_config(nexum_config, provider, &tmp);
    let mut sessions = HashMap::new();
    let transport = MockTransport;

    handle_request(
        "session/set_config_option",
        &json!({"sessionId": "session-a", "configId": "model", "value": "next-model"}),
        None,
        &cfg,
        &mut sessions,
        &transport,
    )
    .await
    .unwrap();
    let identity = handle_request(
        "runtime/identity",
        &json!({}),
        None,
        &cfg,
        &mut sessions,
        &transport,
    )
    .await
    .unwrap();

    assert_eq!(identity["provider"], "OpenAI");
    assert_eq!(identity["model"], "next-model");
}

#[tokio::test]
async fn test_cron_resolve_pending_interaction_deniega_b_y_permite_a() {
    let tmp = tempfile::TempDir::new().unwrap();
    let provider_config = make_provider_config("test", "openai", "test-key", "test-model");
    let mut nexum_config = NexumConfig::default();
    nexum_config.config.active_provider_id = "test".to_string();
    nexum_config.config.providers = vec![provider_config];
    let provider = LlmProvider::from_config(&nexum_config).unwrap();
    let mut cfg = make_server_config(nexum_config, provider, &tmp);
    cfg.pending_interaction_broker = Some(Arc::new(MockPendingInteractionBroker::new(1)));
    let mut sessions = HashMap::new();
    let transport = MockTransport;

    let missing_context = handle_request(
        "cron/resolve_pending_interaction",
        &json!({
            "interactionId": "interaction-1",
            "targetThreadId": "thread-a",
            "decision": "approve",
            "actorId": "client-a",
        }),
        None,
        &cfg,
        &mut sessions,
        &transport,
    )
    .await;

    assert!(
        missing_context.is_err(),
        "sin contexto de caller se debe denegar"
    );

    let caller_b = CallerContext::from_connection(2, None);
    let rejected = handle_request(
        "cron/resolve_pending_interaction",
        &json!({
            "interactionId": "interaction-1",
            "targetThreadId": "thread-a",
            "decision": "approve",
            "actorId": "client-a",
        }),
        Some(&caller_b),
        &cfg,
        &mut sessions,
        &transport,
    )
    .await;

    assert!(
        rejected.is_err(),
        "el cliente B no puede resolver el pending de A"
    );

    let caller_a = CallerContext::from_connection(1, None);
    let response = handle_request(
        "cron/resolve_pending_interaction",
        &json!({
            "interactionId": "interaction-1",
            "targetThreadId": "thread-a",
            "decision": "approve",
        }),
        Some(&caller_a),
        &cfg,
        &mut sessions,
        &transport,
    )
    .await
    .unwrap();

    assert_eq!(response["interaction"]["status"], "Approved");
    assert_eq!(response["continuationSupported"], false);
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

/// 验证 session/update_config 切换 active_provider_id 后 cfg.provider 正确更新
#[tokio::test]
async fn test_update_config_切换provider后cfg_provider更新() {
    // Arrange: 构造两个 provider（a=openai, b=anthropic），初始 active_provider_id = "a"
    let tmp = tempfile::TempDir::new().unwrap();
    let provider_a = make_provider_config("a", "openai", "sk-openai-test", "gpt-4o");
    let provider_b = make_provider_config("b", "anthropic", "sk-ant-test", "claude-sonnet-4-6");

    let mut nexum_config = NexumConfig::default();
    nexum_config.config.active_provider_id = "a".to_string();
    nexum_config.config.active_alias = "sonnet".to_string();
    nexum_config.config.providers = vec![provider_a.clone(), provider_b.clone()];

    let initial_provider = LlmProvider::from_config(&nexum_config).unwrap();
    assert!(
        matches!(initial_provider, LlmProvider::OpenAi { .. }),
        "初始 provider 应为 OpenAI"
    );

    let cfg = make_server_config(nexum_config.clone(), initial_provider, &tmp);
    let mut sessions = HashMap::new();
    let transport = MockTransport;

    // 构造 update_config 参数：active_provider_id 改为 "b"
    let mut updated_config = nexum_config.clone();
    updated_config.config.active_provider_id = "b".to_string();

    let params = json!({
        "sessionId": "test-session",
        "config": updated_config,
    });

    // Act: 调用 handle_request
    let result = handle_request(
        "session/update_config",
        &params,
        None,
        &cfg,
        &mut sessions,
        &transport,
    )
    .await
    .unwrap();

    // Assert: cfg.provider 应切换到 anthropic
    let provider = cfg.provider.read();
    assert!(
        matches!(&*provider, LlmProvider::Anthropic { model, .. } if model == "claude-sonnet-4-6"),
        "切换后 provider 应为 Anthropic claude-sonnet-4-6，实际: display={} model={}",
        provider.display_name(),
        provider.model_name(),
    );
    assert_eq!(
        provider.display_name(),
        "Anthropic",
        "display_name 应为 Anthropic"
    );

    // 验证返回值包含 configOptions
    assert!(
        result.get("configOptions").is_some(),
        "响应应包含 configOptions"
    );
}

#[tokio::test]
async fn test_update_config_actualiza_runtime_identity_provider_y_model() {
    let tmp = tempfile::TempDir::new().unwrap();
    let provider_a = make_provider_config("a", "openai", "sk-openai-test", "gpt-a");
    let provider_b = make_provider_config("b", "anthropic", "sk-ant-test", "claude-b");
    let mut config = NexumConfig::default();
    config.config.active_provider_id = "a".to_string();
    config.config.active_alias = "sonnet".to_string();
    config.config.providers = vec![provider_a, provider_b];
    let provider = LlmProvider::from_config(&config).unwrap();
    let cfg = make_server_config(config.clone(), provider, &tmp);
    let mut sessions = HashMap::new();

    let mut updated = config;
    updated.config.active_provider_id = "b".to_string();
    updated.config.active_alias = "sonnet".to_string();
    handle_request(
        "session/update_config",
        &json!({"sessionId": "session-a", "config": updated}),
        None,
        &cfg,
        &mut sessions,
        &MockTransport,
    )
    .await
    .unwrap();

    let identity = cfg.runtime.identity.read().clone();
    assert_eq!(identity.provider.as_deref(), Some("Anthropic"));
    assert_eq!(identity.model.as_deref(), Some("claude-b"));
}

#[test]
fn test_config_update_notification_without_session_actualiza_runtime_identity_provider_y_model() {
    let tmp = tempfile::TempDir::new().unwrap();
    let provider_a = make_provider_config("a", "openai", "sk-openai-test", "gpt-a");
    let provider_b = make_provider_config("b", "anthropic", "sk-ant-test", "claude-b");
    let mut config = NexumConfig::default();
    config.config.active_provider_id = "a".to_string();
    config.config.active_alias = "sonnet".to_string();
    config.config.providers = vec![provider_a, provider_b];
    let provider = LlmProvider::from_config(&config).unwrap();
    let cfg = make_server_config(config.clone(), provider, &tmp);

    let mut updated = config;
    updated.config.active_provider_id = "b".to_string();
    updated.config.active_alias = "sonnet".to_string();
    crate::server::handle_notification(
        "session/config_update",
        &json!({"config": updated}),
        &HashMap::new(),
        &cfg,
    );

    let identity = cfg.runtime.identity.read().clone();
    assert_eq!(identity.provider.as_deref(), Some("Anthropic"));
    assert_eq!(identity.model.as_deref(), Some("claude-b"));
}

#[test]
fn test_config_update_model_notification_actualiza_runtime_identity() {
    let tmp = tempfile::TempDir::new().unwrap();
    let provider_a = make_provider_config("a", "openai", "sk-openai-test", "gpt-a");
    let mut config = NexumConfig::default();
    config.config.active_provider_id = "a".to_string();
    config.config.active_alias = "sonnet".to_string();
    config.config.providers = vec![provider_a];
    let provider = LlmProvider::from_config(&config).unwrap();
    let cfg = make_server_config(config, provider, &tmp);

    crate::server::handle_notification(
        "session/config_update",
        &json!({"configId": "model", "value": "catalog-model"}),
        &HashMap::new(),
        &cfg,
    );

    let identity = cfg.runtime.identity.read().clone();
    assert_eq!(identity.provider.as_deref(), Some("OpenAI"));
    assert_eq!(identity.model.as_deref(), Some("catalog-model"));
}

/// 验证 session/update_config 空 providers 时返回错误
#[tokio::test]
async fn test_update_config_空providers返回错误() {
    let tmp = tempfile::TempDir::new().unwrap();
    let provider_a = make_provider_config("a", "openai", "sk-openai-test", "gpt-4o");

    let mut nexum_config = NexumConfig::default();
    nexum_config.config.active_provider_id = "a".to_string();
    nexum_config.config.providers = vec![provider_a];

    let initial_provider = LlmProvider::from_config(&nexum_config).unwrap();
    let cfg = make_server_config(nexum_config.clone(), initial_provider, &tmp);
    let mut sessions = HashMap::new();
    let transport = MockTransport;

    // 空 providers
    let mut bad_config = NexumConfig::default();
    bad_config.config.providers = vec![];

    let params = json!({
        "sessionId": "test-session",
        "config": bad_config,
    });

    let result = handle_request(
        "session/update_config",
        &params,
        None,
        &cfg,
        &mut sessions,
        &transport,
    )
    .await;

    assert!(result.is_err(), "空 providers 应返回错误");
    let err = result.unwrap_err();
    assert!(
        err.message.contains("providers cannot be empty"),
        "错误消息应提及 providers 为空，实际: {}",
        err.message,
    );
}

/// 验证 session/update_config 不存在的 active_provider_id 返回错误
#[tokio::test]
async fn test_update_config_不存在的provider_id返回错误() {
    let tmp = tempfile::TempDir::new().unwrap();
    let provider_a = make_provider_config("a", "openai", "sk-openai-test", "gpt-4o");

    let mut nexum_config = NexumConfig::default();
    nexum_config.config.active_provider_id = "a".to_string();
    nexum_config.config.providers = vec![provider_a];

    let initial_provider = LlmProvider::from_config(&nexum_config).unwrap();
    let cfg = make_server_config(nexum_config.clone(), initial_provider, &tmp);
    let mut sessions = HashMap::new();
    let transport = MockTransport;

    // active_provider_id 指向不存在的 provider
    let mut bad_config = nexum_config.clone();
    bad_config.config.active_provider_id = "nonexistent".to_string();
    bad_config.config.providers = vec![make_provider_config(
        "a",
        "openai",
        "sk-openai-test",
        "gpt-4o",
    )];

    let params = json!({
        "sessionId": "test-session",
        "config": bad_config,
    });

    let result = handle_request(
        "session/update_config",
        &params,
        None,
        &cfg,
        &mut sessions,
        &transport,
    )
    .await;

    assert!(result.is_err(), "不存在的 provider_id 应返回错误");
    let err = result.unwrap_err();
    assert!(
        err.message.contains("not found"),
        "错误消息应提及 not found，实际: {}",
        err.message,
    );
}
