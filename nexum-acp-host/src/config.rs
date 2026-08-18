use std::{collections::HashMap, sync::Arc};

use anyhow::Context;
use nexum_acp::{
    cron::{
        CronPromptResources, OwnerPrincipalAuthorizer, SqliteCronStore,
        SqlitePendingInteractionBroker,
    },
    provider::{self, LlmProvider},
    runtime::{
        ResolvedRuntimeInputs, RuntimeIdentityInput, RuntimeTransportKind,
        SharedAcpRuntimeBootstrap, SharedRuntimeInputsResolver, RUNTIME_CAPABILITY_SCHEMA,
        RUNTIME_PROTOCOL_SCHEMA,
    },
    server::{AcpServerConfig, NoopPredictionPolicy},
};
use nexum_agent::thread::{SqliteThreadStore, ThreadStore};
use nexum_middlewares::{
    mcp::{McpClientPool, McpInitStatus},
    plugin::load_runtime_plugin_snapshot,
    prelude::{PermissionMode, SharedPermissionMode},
    tool_search::ToolSearchIndex,
};

/// Recursos del host local. El scheduler cron toma los mismos recursos que el
/// servidor ACP y solo cambia el adaptador de entrada/salida.
pub struct HostConfig {
    pub server: AcpServerConfig,
    pub cron: Option<HostCronConfig>,
}

/// Cron is optional only so isolated local-host tests do not create a scheduler.
/// Production configuration always supplies it.
pub struct HostCronConfig {
    pub prompt_resources: CronPromptResources,
    pub store: Arc<SqliteCronStore>,
}

/// Construye exactamente los recursos consumidos por el executor compartido.
/// No resuelve providers, tools o persistencia por una ruta alternativa.
pub async fn load_host_config() -> anyhow::Result<HostConfig> {
    let nexum_config = provider::load().unwrap_or_default();
    let provider = LlmProvider::from_config(&nexum_config)
        .or_else(LlmProvider::from_env)
        .context("no configured ACP provider")?;
    let thread_store: Arc<dyn ThreadStore> = Arc::new(
        SqliteThreadStore::default_path()
            .await
            .context("open shared thread store")?,
    );
    // Las corridas cron no tienen un cliente al que pedir permisos. Default +
    // HeadlessFailSafeBroker garantiza que una operación sensible se rechace.
    let permission_mode = SharedPermissionMode::new(PermissionMode::Default);
    let provider = Arc::new(parking_lot::RwLock::new(provider));
    let nexum_config = Arc::new(parking_lot::RwLock::new(nexum_config));
    let tool_search_index = Arc::new(ToolSearchIndex::new());
    let shared_tools = Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let cwd = std::env::current_dir().ok();
    let workspace = cwd.as_ref().map(|path| path.to_string_lossy().to_string());
    let cwd = cwd.unwrap_or_else(|| std::path::PathBuf::from("."));
    let claude_dir = dirs_next::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".claude");
    let plugin_snapshot = load_runtime_plugin_snapshot(&claude_dir, &cwd.to_string_lossy());
    let mcp_pool = Arc::new(McpClientPool::new_pending());
    let (mcp_status_tx, _mcp_status_rx) = tokio::sync::watch::channel(McpInitStatus::Pending);
    // P0 fix (NEXUM_TUI_MCP_INIT_DEADLOCK): NO bloquear el arranque del host —
    // ni, por transitividad, el primer turno interactivo— esperando a que TODOS
    // los MCP servers inicialicen serialmente. Con un MCP opcional lento (p. ej.
    // `npx -y @anyproto/anytype-mcp`, que descarga en el primer arranque), el
    // `.await` inline colgaba el host antes de servir prompts.
    //
    // El pool es un `Arc` compartido con interior-mutability: `run_initialize`
    // lo va poblando en background y `build_tool_bridges(pool)` toma lo que esté
    // listo al construir el agente. Un turno que no usa tools de MCP no depende
    // de esta inicialización. La TUI ya inicializa el pool en background
    // (nexum-tui/src/app/mod.rs); acá alineamos el host al mismo patrón.
    {
        let mcp_pool = mcp_pool.clone();
        let cwd = cwd.clone();
        let claude_dir = claude_dir.clone();
        tokio::spawn(async move {
            McpClientPool::run_initialize(
                mcp_pool,
                &cwd,
                &claude_dir,
                mcp_status_tx,
                None,
                None,
            )
            .await;
        });
    }
    let channel_state = Some(nexum_agent::interaction::ChannelState::new());
    let cron_store = Arc::new(
        SqliteCronStore::open(crate::lifecycle::default_cron_store_path()?)
            .await
            .context("open local cron store")?,
    );
    #[cfg(target_os = "linux")]
    let pending_interaction_broker = Some(Arc::new(
        SqlitePendingInteractionBroker::new(cron_store.clone())
            .with_authorizer(Arc::new(OwnerPrincipalAuthorizer)),
    )
        as Arc<dyn nexum_acp::cron::PendingInteractionBroker>);
    #[cfg(not(target_os = "linux"))]
    let pending_interaction_broker = None;
    let active_provider = nexum_config
        .read()
        .config
        .providers
        .iter()
        .find(|candidate| candidate.id == nexum_config.read().config.active_provider_id)
        .map(|candidate| candidate.provider_type.clone());
    let runtime = SharedAcpRuntimeBootstrap::build(
        SharedRuntimeInputsResolver::new(ResolvedRuntimeInputs {
            provider: provider.clone(),
            nexum_config: nexum_config.clone(),
            permission_mode: permission_mode.clone(),
            thread_store: thread_store.clone(),
            config_path: provider::config_path(),
            // The local host has no Cron control protocol in this slice. The
            // shared bootstrap deliberately preserves CronUnavailable instead
            // of creating a fallback scheduler.
            cron_control: None,
            mcp_pool: Some(mcp_pool.clone()),
            channel_state: channel_state.clone(),
            plugin_snapshot,
            tool_search_index,
            shared_tools,
            langfuse_session: None,
            prediction_policy: Arc::new(NoopPredictionPolicy),
            pending_interaction_broker,
            identity: RuntimeIdentityInput {
                protocol_schema: RUNTIME_PROTOCOL_SCHEMA.to_string(),
                capability_schema: RUNTIME_CAPABILITY_SCHEMA.to_string(),
                transport_kind: RuntimeTransportKind::Unix,
                pid: Some(std::process::id()),
                host_principal: None,
                provider: active_provider,
                model: Some(provider.read().model_name().to_string()),
                workspace,
                started_at: chrono::Utc::now(),
            },
        })
        .resolve(),
    )?;
    let cron_prompt_resources = CronPromptResources {
        provider,
        nexum_config,
        permission_mode,
        cron_control: nexum_acp::cron::CronControlClient::unavailable(),
        mcp_pool: runtime.server.mcp_pool.clone(),
        channel_state: runtime.server.channel_state.clone(),
        plugin_skill_roots: runtime.server.plugin_skill_roots.clone(),
        plugin_agent_dirs: runtime.server.plugin_agent_dirs.clone(),
        hook_groups: runtime.server.hook_groups.clone(),
        plugin_lsp_servers: runtime.server.plugin_lsp_servers.clone(),
        tool_search_index: runtime.server.tool_search_index.clone(),
        shared_tools: runtime.server.shared_tools.clone(),
        thread_store,
        langfuse_session: None,
        session_manager: runtime.server.session_manager.clone(),
        interaction_policy: nexum_acp::cron::InteractionPolicy::FailSafely,
        interaction_sink: cron_store.clone(),
    };
    Ok(HostConfig {
        server: runtime.server,
        cron: Some(HostCronConfig {
            prompt_resources: cron_prompt_resources,
            store: cron_store,
        }),
    })
}
