use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use nexum_agent::{interaction::ChannelState, thread::ThreadStore, tools::BaseTool};
use nexum_middlewares::{
    cron::CronControlClient, mcp::McpClientPool, plugin::RuntimePluginSnapshot,
    prelude::SharedPermissionMode, tool_search::ToolSearchIndex,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    cron::PendingInteractionBroker,
    langfuse::LangfuseSession,
    provider::{LlmProvider, NexumConfig},
    server::{AcpServerConfig, PredictionPolicy},
    session::SessionManager,
    transport::types::HostPrincipal,
};

use super::{sanitize, CapabilityState, RuntimeCapabilities};

pub const RUNTIME_PROTOCOL_SCHEMA: &str = "nexum.acp.runtime/v1";
pub const RUNTIME_CAPABILITY_SCHEMA: &str = "nexum.acp.capabilities/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTransportKind {
    Mpsc,
    Unix,
    Stdio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeIdentity {
    pub runtime_instance_id: String,
    pub protocol_schema: String,
    pub capability_schema: String,
    pub transport_kind: RuntimeTransportKind,
    pub pid: Option<u32>,
    pub host_principal: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub workspace: Option<String>,
    pub started_at: DateTime<Utc>,
}

pub struct RuntimeIdentityInput {
    pub protocol_schema: String,
    pub capability_schema: String,
    pub transport_kind: RuntimeTransportKind,
    pub pid: Option<u32>,
    pub host_principal: Option<HostPrincipal>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub workspace: Option<String>,
    pub started_at: DateTime<Utc>,
}

impl RuntimeIdentity {
    pub fn new(input: RuntimeIdentityInput) -> Self {
        Self {
            runtime_instance_id: Uuid::new_v4().to_string(),
            protocol_schema: sanitize(input.protocol_schema),
            capability_schema: sanitize(input.capability_schema),
            transport_kind: input.transport_kind,
            pid: input.pid,
            host_principal: input
                .host_principal
                .map(|principal| sanitize(principal.as_str())),
            provider: sanitize_optional(input.provider),
            model: sanitize_optional(input.model),
            workspace: sanitize_optional(input.workspace),
            started_at: input.started_at,
        }
    }

    pub fn update_provider(&mut self, provider: &LlmProvider) {
        self.provider = sanitize_optional(Some(provider.display_name().to_string()));
        self.model = sanitize_optional(Some(provider.model_name().to_string()));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHealthState {
    Starting,
    Ready,
    Degraded,
    ShuttingDown,
}

#[derive(Clone)]
pub struct RuntimeHealth(Arc<parking_lot::RwLock<RuntimeHealthState>>);

impl RuntimeHealth {
    pub fn new(state: RuntimeHealthState) -> Self {
        Self(Arc::new(parking_lot::RwLock::new(state)))
    }

    pub fn get(&self) -> RuntimeHealthState {
        *self.0.read()
    }

    pub fn set(&self, state: RuntimeHealthState) {
        *self.0.write() = state;
    }
}

/// Inputs resolved by a host before it enters the shared ACP runtime. This type
/// deliberately owns no configuration discovery, plugin loading, scheduler, or
/// secondary provider/thread-store construction.
pub struct ResolvedRuntimeInputs {
    pub provider: Arc<parking_lot::RwLock<LlmProvider>>,
    pub nexum_config: Arc<parking_lot::RwLock<NexumConfig>>,
    pub permission_mode: Arc<SharedPermissionMode>,
    pub thread_store: Arc<dyn ThreadStore>,
    pub config_path: PathBuf,
    pub cron_control: Option<CronControlClient>,
    pub mcp_pool: Option<Arc<McpClientPool>>,
    pub channel_state: Option<Arc<ChannelState>>,
    pub plugin_snapshot: RuntimePluginSnapshot,
    pub tool_search_index: Arc<ToolSearchIndex>,
    pub shared_tools: Arc<parking_lot::RwLock<HashMap<String, Arc<dyn BaseTool>>>>,
    pub langfuse_session: Option<Arc<LangfuseSession>>,
    pub prediction_policy: Arc<dyn PredictionPolicy>,
    pub pending_interaction_broker: Option<Arc<dyn PendingInteractionBroker>>,
    pub identity: RuntimeIdentityInput,
}

/// Host-neutral assembly point. Hosts resolve their own resources first, then
/// hand the resulting values to this resolver without triggering side effects.
pub struct SharedRuntimeInputsResolver {
    inputs: ResolvedRuntimeInputs,
}

impl SharedRuntimeInputsResolver {
    pub fn new(inputs: ResolvedRuntimeInputs) -> Self {
        Self { inputs }
    }

    pub fn resolve(self) -> ResolvedRuntimeInputs {
        self.inputs
    }
}

pub struct SharedAcpRuntime {
    pub server: AcpServerConfig,
    pub identity: RuntimeIdentity,
    pub capabilities: RuntimeCapabilities,
    pub health: RuntimeHealth,
}

/// Validates resolved resources and assembles the transport-neutral ACP server.
/// It only consumes handles supplied by the host; it never reads user config or
/// constructs a Cron runtime, provider, thread store, socket, or plugin loader.
pub struct SharedAcpRuntimeBootstrap;

impl SharedAcpRuntimeBootstrap {
    pub fn build(inputs: ResolvedRuntimeInputs) -> Result<SharedAcpRuntime> {
        let identity = RuntimeIdentity::new(inputs.identity);
        if identity.protocol_schema.is_empty() || identity.capability_schema.is_empty() {
            bail!("runtime schemas cannot be empty")
        }
        if inputs.config_path.as_os_str().is_empty() {
            bail!("runtime config path cannot be empty")
        }
        if inputs.provider.read().model_name().trim().is_empty() {
            bail!("runtime provider model cannot be empty")
        }

        let cron_available = inputs.cron_control.is_some();
        let cron_unavailable_reason = if identity.transport_kind == RuntimeTransportKind::Mpsc {
            "CronUnavailable: MPSC is an explicit unshared runtime; no cron host is connected"
        } else {
            "no cron host is connected"
        };
        let capabilities = RuntimeCapabilities::new(
            identity.capability_schema.clone(),
            [
                ("provider", CapabilityState::Available),
                ("thread_store", CapabilityState::Available),
                ("tool_index", CapabilityState::Available),
                (
                    "cron",
                    if cron_available {
                        CapabilityState::Available
                    } else {
                        CapabilityState::Unavailable {
                            reason: cron_unavailable_reason.to_string(),
                        }
                    },
                ),
                (
                    "mcp",
                    option_state(
                        inputs.mcp_pool.is_some(),
                        "MCP pool was not supplied by the host",
                    ),
                ),
                (
                    "channel_state",
                    option_state(
                        inputs.channel_state.is_some(),
                        "channel state was not supplied by the host",
                    ),
                ),
                (
                    "hooks",
                    option_state(
                        !inputs.plugin_snapshot.hook_groups.is_empty(),
                        "no hooks were discovered for this runtime",
                    ),
                ),
                (
                    "lsp",
                    option_state(
                        !inputs.plugin_snapshot.lsp_servers.is_empty(),
                        "no plugin LSP servers were discovered for this runtime",
                    ),
                ),
                (
                    "pending_interactions",
                    option_state(
                        inputs.pending_interaction_broker.is_some(),
                        "durable interaction broker was not supplied by the host",
                    ),
                ),
            ],
            [
                (
                    "plugin_skill_roots",
                    inputs
                        .plugin_snapshot
                        .skill_roots
                        .iter()
                        .map(|root| root.path.to_string_lossy().to_string())
                        .collect(),
                ),
                (
                    "plugin_agent_dirs",
                    inputs
                        .plugin_snapshot
                        .agent_dirs
                        .iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect(),
                ),
                (
                    "plugin_lsp_servers",
                    inputs
                        .plugin_snapshot
                        .lsp_servers
                        .iter()
                        .map(|server| server.name.clone())
                        .collect(),
                ),
                (
                    "hooks",
                    inputs
                        .plugin_snapshot
                        .hooks
                        .iter()
                        .map(|hook| format!("{:?}", hook.event))
                        .collect(),
                ),
            ],
        );
        let health = RuntimeHealth::new(RuntimeHealthState::Starting);
        let session_manager = SessionManager::new(
            inputs.thread_store.clone(),
            inputs.provider.read().clone(),
            Arc::new(inputs.nexum_config.read().clone()),
            inputs.permission_mode.clone(),
            None,
        );
        let server = AcpServerConfig {
            provider: inputs.provider,
            nexum_config: inputs.nexum_config,
            permission_mode: inputs.permission_mode,
            cron_control: inputs
                .cron_control
                .unwrap_or_else(CronControlClient::unavailable),
            mcp_pool: inputs.mcp_pool,
            channel_state: inputs.channel_state,
            plugin_skill_roots: inputs.plugin_snapshot.skill_roots,
            plugin_agent_dirs: inputs.plugin_snapshot.agent_dirs,
            plugin_hooks: inputs.plugin_snapshot.hooks,
            hook_groups: inputs.plugin_snapshot.hook_groups,
            plugin_lsp_servers: inputs.plugin_snapshot.lsp_servers,
            tool_search_index: inputs.tool_search_index,
            shared_tools: inputs.shared_tools,
            thread_store: inputs.thread_store,
            langfuse_session: inputs.langfuse_session,
            config_path: inputs.config_path,
            session_manager,
            prediction_policy: inputs.prediction_policy,
            pending_interaction_broker: inputs.pending_interaction_broker,
            runtime: RuntimeMetadata {
                identity: Arc::new(parking_lot::RwLock::new(identity.clone())),
                capabilities: capabilities.clone(),
                health: health.clone(),
            },
        };
        Ok(SharedAcpRuntime {
            server,
            identity,
            capabilities,
            health,
        })
    }
}

#[derive(Clone)]
pub struct RuntimeMetadata {
    pub identity: Arc<parking_lot::RwLock<RuntimeIdentity>>,
    pub capabilities: RuntimeCapabilities,
    pub health: RuntimeHealth,
}

impl Default for RuntimeIdentityInput {
    fn default() -> Self {
        Self {
            protocol_schema: RUNTIME_PROTOCOL_SCHEMA.to_string(),
            capability_schema: RUNTIME_CAPABILITY_SCHEMA.to_string(),
            transport_kind: RuntimeTransportKind::Mpsc,
            pid: None,
            host_principal: None,
            provider: None,
            model: None,
            workspace: None,
            started_at: Utc::now(),
        }
    }
}

fn option_state(available: bool, reason: &str) -> CapabilityState {
    if available {
        CapabilityState::Available
    } else {
        CapabilityState::Unavailable {
            reason: reason.to_string(),
        }
    }
}

fn sanitize_optional(value: Option<String>) -> Option<String> {
    value.map(sanitize).filter(|value| !value.is_empty())
}
