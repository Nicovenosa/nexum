use std::{collections::HashMap, path::PathBuf, sync::Arc};

use chrono::Utc;
use nexum_agent::thread::{FilesystemThreadStore, ThreadStore};
use nexum_middlewares::{
    hooks::{HookEvent, HookType, RegisteredHook},
    mcp::McpClientPool,
    plugin::RuntimePluginSnapshot,
    prelude::{PermissionMode, SharedPermissionMode},
    tool_search::ToolSearchIndex,
};

use super::{
    CapabilityState, ResolvedRuntimeInputs, RuntimeCapabilities, RuntimeIdentity,
    RuntimeIdentityInput, RuntimeTransportKind, SharedAcpRuntimeBootstrap,
    SharedRuntimeInputsResolver,
};
use crate::{
    provider::{LlmProvider, NexumConfig, ProviderConfig, ProviderModels},
    server::NoopPredictionPolicy,
    transport::types::HostPrincipal,
};

#[test]
fn test_runtime_identity_sanitiza_campos_opcionales_y_conserva_instancia() {
    let identity = RuntimeIdentity::new(RuntimeIdentityInput {
        protocol_schema: " nexum.acp/v1\n".to_string(),
        capability_schema: " nexum.runtime.capabilities/v1\t".to_string(),
        transport_kind: RuntimeTransportKind::Mpsc,
        pid: Some(42),
        host_principal: Some(HostPrincipal::new(" unix-uid:1000 ").unwrap()),
        provider: Some(" openai\ninternal ".to_string()),
        model: Some(" model\tname ".to_string()),
        workspace: Some(" /workspace/project\n ".to_string()),
        started_at: Utc::now(),
    });

    assert_eq!(identity.protocol_schema, "nexum.acp/v1");
    assert_eq!(identity.capability_schema, "nexum.runtime.capabilities/v1");
    assert_eq!(identity.host_principal.as_deref(), Some("unix-uid:1000"));
    assert_eq!(identity.provider.as_deref(), Some("openai internal"));
    assert_eq!(identity.model.as_deref(), Some("model name"));
    assert_eq!(identity.workspace.as_deref(), Some("/workspace/project"));
    assert!(!identity.runtime_instance_id.is_empty());
    assert_eq!(identity.pid, Some(42));
    assert_eq!(
        identity.runtime_instance_id,
        identity.clone().runtime_instance_id
    );

    let restarted = RuntimeIdentity::new(RuntimeIdentityInput::default());
    assert_ne!(identity.runtime_instance_id, restarted.runtime_instance_id);
}

#[test]
fn test_runtime_capabilities_hashea_json_canonico_con_colecciones_ordenadas() {
    let first = RuntimeCapabilities::new(
        "nexum.runtime.capabilities/v1",
        [
            (
                "cron",
                CapabilityState::Unavailable {
                    reason: "no host".to_string(),
                },
            ),
            ("provider", CapabilityState::Available),
        ],
        [(
            "tools",
            vec!["write".to_string(), "bash".to_string(), "bash".to_string()],
        )],
    );
    let second = RuntimeCapabilities::new(
        "nexum.runtime.capabilities/v1",
        [
            ("provider", CapabilityState::Available),
            (
                "cron",
                CapabilityState::Unavailable {
                    reason: "no host".to_string(),
                },
            ),
        ],
        [("tools", vec!["bash".to_string(), "write".to_string()])],
    );

    assert_eq!(first.hash, second.hash);
    assert_eq!(first.collections["tools"], ["bash", "write"]);
    assert_eq!(
        first.resources["cron"],
        CapabilityState::Unavailable {
            reason: "no host".to_string()
        }
    );
}

#[test]
fn test_runtime_capabilities_expone_razones_de_recursos_no_disponibles() {
    let capabilities = RuntimeCapabilities::new(
        "nexum.runtime.capabilities/v1",
        [(
            "cron",
            CapabilityState::Unavailable {
                reason: "no cron host is connected".to_string(),
            },
        )],
        std::iter::empty::<(&str, Vec<String>)>(),
    );

    assert_eq!(
        capabilities.resources["cron"],
        CapabilityState::Unavailable {
            reason: "no cron host is connected".to_string(),
        }
    );
}

#[test]
fn test_runtime_crate_no_depende_de_nexum_tui() {
    let manifest =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .unwrap();

    assert!(!manifest.contains("nexum-tui"));
}

#[tokio::test]
async fn test_mpsc_bootstrap_declares_cron_unavailable() {
    let temp = tempfile::TempDir::new().unwrap();
    let runtime = SharedAcpRuntimeBootstrap::build(
        SharedRuntimeInputsResolver::new(make_inputs(&temp)).resolve(),
    )
    .unwrap();

    assert_eq!(
        runtime.capabilities.resources["cron"],
        CapabilityState::Unavailable {
            reason:
                "CronUnavailable: MPSC is an explicit unshared runtime; no cron host is connected"
                    .to_string(),
        }
    );
    assert!(matches!(
        runtime.server.cron_control.list().await,
        Err(nexum_middlewares::cron::CronControlError::Unavailable(_))
    ));
}

#[test]
fn test_runtime_reuses_host_handles_and_advertises_only_supplied_resources() {
    let temp = tempfile::TempDir::new().unwrap();
    let inputs = make_inputs(&temp);
    let provider = inputs.provider.clone();
    let thread_store = inputs.thread_store.clone();
    let runtime =
        SharedAcpRuntimeBootstrap::build(SharedRuntimeInputsResolver::new(inputs).resolve())
            .unwrap();

    assert!(Arc::ptr_eq(&runtime.server.provider, &provider));
    assert!(Arc::ptr_eq(&runtime.server.thread_store, &thread_store));
    assert!(matches!(
        runtime.capabilities.resources["mcp"],
        CapabilityState::Unavailable { .. }
    ));
    assert!(matches!(
        runtime.capabilities.resources["hooks"],
        CapabilityState::Unavailable { .. }
    ));
    assert!(matches!(
        runtime.capabilities.resources["lsp"],
        CapabilityState::Unavailable { .. }
    ));
}

#[test]
fn test_runtime_capabilities_advertise_mcp_plugin_lsp_and_hooks_from_fixture() {
    let temp = tempfile::TempDir::new().unwrap();
    let mut inputs = make_inputs(&temp);
    inputs.mcp_pool = Some(Arc::new(McpClientPool::new_pending()));
    let hook = RegisteredHook {
        hook: HookType::Command {
            command: "true".to_string(),
            shell: None,
            timeout: None,
            status_message: None,
            once: false,
            async_run: false,
            async_rewake: false,
            matcher: None,
            condition: None,
        },
        event: HookEvent::PreToolUse,
        matcher: None,
        plugin_name: "fixture".to_string(),
        plugin_id: "fixture@test".to_string(),
        plugin_root: temp.path().to_path_buf(),
        plugin_data_dir: temp.path().to_path_buf(),
        plugin_options: HashMap::new(),
    };
    inputs.plugin_snapshot = RuntimePluginSnapshot {
        skill_roots: Vec::new(),
        agent_dirs: Vec::new(),
        hooks: vec![hook.clone()],
        hook_groups: vec![vec![hook]],
        lsp_servers: vec![nexum_lsp::config::LspServerConfig {
            name: "fixture-lsp".to_string(),
            command: "fixture-lsp".to_string(),
            args: Vec::new(),
            env: None,
            extension_to_language: HashMap::new(),
            initialization_options: None,
            disabled: None,
            max_restarts: None,
            startup_timeout: None,
            source: None,
        }],
    };

    let runtime =
        SharedAcpRuntimeBootstrap::build(SharedRuntimeInputsResolver::new(inputs).resolve())
            .unwrap();

    assert_eq!(
        runtime.capabilities.resources["mcp"],
        CapabilityState::Available
    );
    assert_eq!(
        runtime.capabilities.resources["hooks"],
        CapabilityState::Available
    );
    assert_eq!(
        runtime.capabilities.resources["lsp"],
        CapabilityState::Available
    );
    assert_eq!(runtime.capabilities.collections["hooks"], ["PreToolUse"]);
    assert_eq!(
        runtime.capabilities.collections["plugin_lsp_servers"],
        ["fixture-lsp"]
    );
}

fn make_inputs(temp: &tempfile::TempDir) -> ResolvedRuntimeInputs {
    let mut config = NexumConfig::default();
    config.config.active_provider_id = "test".to_string();
    config.config.providers = vec![ProviderConfig {
        id: "test".to_string(),
        provider_type: "openai".to_string(),
        api_key: "test-key".to_string(),
        models: ProviderModels {
            sonnet: "test-model".to_string(),
            ..Default::default()
        },
        ..Default::default()
    }];
    let provider = LlmProvider::from_config(&config).unwrap();
    let thread_store: Arc<dyn ThreadStore> = Arc::new(FilesystemThreadStore::new(temp.path()));
    ResolvedRuntimeInputs {
        provider: Arc::new(parking_lot::RwLock::new(provider)),
        nexum_config: Arc::new(parking_lot::RwLock::new(config)),
        permission_mode: SharedPermissionMode::new(PermissionMode::Bypass),
        thread_store,
        config_path: temp.path().join("config.json"),
        cron_control: None,
        mcp_pool: None,
        channel_state: None,
        plugin_snapshot: RuntimePluginSnapshot::default(),
        tool_search_index: Arc::new(ToolSearchIndex::new()),
        shared_tools: Arc::new(parking_lot::RwLock::new(HashMap::new())),
        langfuse_session: None,
        prediction_policy: Arc::new(NoopPredictionPolicy),
        pending_interaction_broker: None,
        identity: RuntimeIdentityInput::default(),
    }
}
