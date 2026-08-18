//! Turno de voz que usa ACP como único límite de ejecución remota.
//!
//! Esta capa no construye runtimes, proveedores, herramientas ni almacenamiento
//! conversacional. Solo conserva metadata de sesión de voz y reusa el cliente
//! ACP genérico para transport y notificaciones.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nexum_acp::{
    task::{TaskEnvelopeV1, TaskSource},
    transport::types::{AcpError, RequestId},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};

use crate::acp_client::{
    bootstrap::{connect_local_client, AcpTransportMode},
    AcpClientTransport, AcpNotification, AcpTuiClient,
};

use super::VoiceResponse;

const DEFAULT_TURN_WAIT: Duration = Duration::from_secs(30);
const MAX_TURN_WAIT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VoiceEscalationPolicy {
    Local,
    #[default]
    Ask,
    Smart,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoicePreferences {
    pub schema_version: u32,
    #[serde(default)]
    pub policy: VoiceEscalationPolicy,
    #[serde(default)]
    pub allow_tools: bool,
    #[serde(default)]
    pub allow_paid_providers: bool,
    #[serde(default)]
    pub max_cost_micros: Option<u64>,
    #[serde(default)]
    pub previous_model: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
}

impl Default for VoicePreferences {
    fn default() -> Self {
        Self {
            schema_version: 1,
            policy: VoiceEscalationPolicy::Ask,
            allow_tools: false,
            allow_paid_providers: false,
            max_cost_micros: None,
            previous_model: None,
            default_model: None,
        }
    }
}

#[derive(Clone)]
pub struct VoicePreferencesStore {
    path: PathBuf,
}

impl VoicePreferencesStore {
    pub fn default_path() -> PathBuf {
        nexum_agent::config_home::nexum_home().join("voice/preferences.json")
    }

    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> VoicePreferences {
        match std::fs::read(&self.path)
            .ok()
            .and_then(|data| serde_json::from_slice::<VoicePreferences>(&data).ok())
            .filter(|preferences| preferences.schema_version == 1)
        {
            Some(preferences) => preferences,
            None => {
                if self.path.exists() {
                    let recovered = self
                        .path
                        .with_file_name(format!("preferences.corrupt-{}.json", now_epoch_ms()));
                    let _ = std::fs::rename(&self.path, recovered);
                }
                VoicePreferences::default()
            }
        }
    }

    pub fn save(&self, preferences: &VoicePreferences) -> Result<(), VoiceTurnError> {
        if preferences.schema_version != 1 {
            return Err(VoiceTurnError::InvalidPreferencesVersion(
                preferences.schema_version,
            ));
        }
        write_atomic_json(&self.path, preferences)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingVoiceActionKind {
    Escalation,
    Model,
    ToolPermission,
    CostConfirmation,
    Context,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingVoiceActionStatus {
    #[default]
    Pending,
    Approved,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VoicePolicyCapabilities {
    pub provider_available: bool,
    pub provider_is_paid: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tools_supported: bool,
    pub estimated_cost_micros: Option<u64>,
}

pub struct VoicePolicyGate;

impl VoicePolicyGate {
    pub fn route(
        preferences: &VoicePreferences,
        envelope: TaskEnvelopeV1,
        capabilities: &VoicePolicyCapabilities,
    ) -> VoiceRouteDecision {
        let provider = capabilities
            .provider
            .clone()
            .unwrap_or_else(|| "runtime configurado".into());
        let model = capabilities
            .model
            .clone()
            .unwrap_or_else(|| "modelo configurado".into());
        if preferences.policy == VoiceEscalationPolicy::Local {
            return VoiceRouteDecision::Blocked {
                reason: "La política de voz permite solo resolución local.".into(),
            };
        }
        if preferences.policy == VoiceEscalationPolicy::Ask {
            return VoiceRouteDecision::AskForEscalation {
                envelope,
                provider,
                model,
                reason: "La política de voz requiere confirmación para escalar.".into(),
            };
        }
        if !capabilities.provider_available {
            return VoiceRouteDecision::Blocked {
                reason: "El runtime no declaró un proveedor disponible para este turno.".into(),
            };
        }
        if !envelope.allowed_tools.is_empty() {
            if !capabilities.tools_supported {
                return VoiceRouteDecision::Blocked {
                    reason: "El runtime no declaró soporte para las herramientas solicitadas."
                        .into(),
                };
            }
            if !preferences.allow_tools {
                let tools = envelope.allowed_tools.clone();
                return VoiceRouteDecision::ToolPermissionRequired {
                    envelope,
                    tools,
                    reason: "La política de voz requiere autorización de herramientas.".into(),
                };
            }
        }
        let exceeds_budget = match (
            preferences.max_cost_micros,
            capabilities.estimated_cost_micros,
        ) {
            (Some(limit), Some(estimated)) => estimated > limit,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if (capabilities.provider_is_paid && !preferences.allow_paid_providers) || exceeds_budget {
            return VoiceRouteDecision::CostConfirmationRequired {
                envelope,
                provider,
                model,
                estimated_cost_micros: capabilities.estimated_cost_micros.unwrap_or_default(),
                reason: "La política de voz requiere confirmación de costo.".into(),
            };
        }
        VoiceRouteDecision::Escalate {
            envelope,
            reason: "La política inteligente autorizó la escalación declarada por Hormiguero."
                .into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingVoiceAction {
    pub id: String,
    pub session_id: String,
    pub request_id: Option<String>,
    pub kind: PendingVoiceActionKind,
    pub status: PendingVoiceActionStatus,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

impl PendingVoiceAction {
    pub fn new(
        id: impl Into<String>,
        session_id: impl Into<String>,
        request_id: Option<String>,
        kind: PendingVoiceActionKind,
        created_at_ms: u64,
        ttl_ms: u64,
    ) -> Self {
        Self {
            id: id.into(),
            session_id: session_id.into(),
            request_id,
            kind,
            status: PendingVoiceActionStatus::Pending,
            created_at_ms,
            expires_at_ms: created_at_ms.saturating_add(ttl_ms),
        }
    }

    fn is_active_at(&self, now_ms: u64) -> bool {
        self.status == PendingVoiceActionStatus::Pending && now_ms < self.expires_at_ms
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingVoiceActions {
    actions: Vec<PendingVoiceAction>,
}

impl PendingVoiceActions {
    pub fn restore_valid(actions: Vec<PendingVoiceAction>, now_ms: u64) -> Self {
        Self {
            actions: actions
                .into_iter()
                .filter(|action| action.is_active_at(now_ms))
                .collect(),
        }
    }

    pub fn add(&mut self, action: PendingVoiceAction) {
        self.actions.push(action);
    }

    pub fn active(&self) -> Vec<&PendingVoiceAction> {
        self.actions
            .iter()
            .filter(|action| action.status == PendingVoiceActionStatus::Pending)
            .collect()
    }

    pub fn resolve_by_id(
        &mut self,
        id: &str,
        status: PendingVoiceActionStatus,
        now_ms: u64,
    ) -> Option<String> {
        self.resolve_action_by_id(id, status, now_ms)
            .map(|action| action.id)
    }

    pub fn resolve_action_by_id(
        &mut self,
        id: &str,
        status: PendingVoiceActionStatus,
        now_ms: u64,
    ) -> Option<PendingVoiceAction> {
        let action = self
            .actions
            .iter_mut()
            .find(|action| action.id == id && action.is_active_at(now_ms))?;
        action.status = status;
        Some(action.clone())
    }

    pub fn resolve_by_session_request_action(
        &mut self,
        session_id: &str,
        request_id: Option<&str>,
        kind: PendingVoiceActionKind,
        status: PendingVoiceActionStatus,
        now_ms: u64,
    ) -> Option<String> {
        let action = self.actions.iter_mut().find(|action| {
            action.session_id == session_id
                && action.request_id.as_deref() == request_id
                && action.kind == kind
                && action.is_active_at(now_ms)
        })?;
        action.status = status;
        Some(action.id.clone())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceRouteDecision {
    Local {
        response: VoiceResponse,
        reason: String,
    },
    Escalate {
        envelope: TaskEnvelopeV1,
        reason: String,
    },
    AskForEscalation {
        envelope: TaskEnvelopeV1,
        provider: String,
        model: String,
        reason: String,
    },
    ToolPermissionRequired {
        envelope: TaskEnvelopeV1,
        tools: Vec<String>,
        reason: String,
    },
    CostConfirmationRequired {
        envelope: TaskEnvelopeV1,
        provider: String,
        model: String,
        estimated_cost_micros: u64,
        reason: String,
    },
    ModelDirective {
        directive: super::model_directive::ModelDirective,
    },
    NeedMoreContext {
        missing_fields: Vec<String>,
    },
    Blocked {
        reason: String,
    },
    Failed {
        error: String,
    },
}

impl VoiceRouteDecision {
    pub fn validate(self) -> Result<Self, VoiceTurnError> {
        match &self {
            Self::ToolPermissionRequired { tools, .. } if tools.is_empty() => {
                return Err(VoiceTurnError::InvalidRoute);
            }
            Self::Escalate { envelope, reason }
            | Self::AskForEscalation {
                envelope, reason, ..
            }
            | Self::ToolPermissionRequired {
                envelope, reason, ..
            }
            | Self::CostConfirmationRequired {
                envelope, reason, ..
            } => {
                if envelope.source != TaskSource::Voice
                    || envelope.user_input.trim().is_empty()
                    || envelope.objective.trim().is_empty()
                    || reason.trim().is_empty()
                {
                    return Err(VoiceTurnError::InvalidRoute);
                }
            }
            Self::Local { reason, .. }
            | Self::Blocked { reason }
            | Self::Failed { error: reason }
                if reason.trim().is_empty() =>
            {
                return Err(VoiceTurnError::InvalidRoute);
            }
            Self::NeedMoreContext { missing_fields } if missing_fields.is_empty() => {
                return Err(VoiceTurnError::InvalidRoute);
            }
            _ => {}
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoiceSessionState {
    pub schema_version: u32,
    pub session_id: String,
    pub thread_id: String,
    pub workspace_id: String,
    pub last_runtime_instance_id: String,
    pub last_result_id: Option<String>,
    pub updated_at: u64,
}

/// Almacena exclusivamente metadata de reconexión. Nunca contiene historial,
/// mensajes, provider state ni handles de runtime.
#[derive(Clone, Default)]
pub struct VoiceSessionStore {
    metadata: Arc<parking_lot::Mutex<Option<VoiceSessionState>>>,
    metadata_path: Option<PathBuf>,
}

impl VoiceSessionStore {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let metadata = std::fs::read(&path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok());
        Self {
            metadata: Arc::new(parking_lot::Mutex::new(metadata)),
            metadata_path: Some(path),
        }
    }

    pub fn get(&self) -> Option<VoiceSessionState> {
        self.metadata.lock().clone()
    }

    pub fn replace(&self, mut metadata: VoiceSessionState) -> Result<(), VoiceTurnError> {
        metadata.updated_at = now_epoch_ms();
        if let Some(path) = &self.metadata_path {
            write_atomic_metadata(path, &metadata)?;
        }
        *self.metadata.lock() = Some(metadata);
        Ok(())
    }

    pub fn clear(&self) -> Result<(), VoiceTurnError> {
        if let Some(path) = &self.metadata_path {
            if path.exists() {
                std::fs::remove_file(path).map_err(VoiceTurnError::Metadata)?;
            }
        }
        *self.metadata.lock() = None;
        Ok(())
    }
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn write_atomic_metadata(path: &Path, metadata: &VoiceSessionState) -> Result<(), VoiceTurnError> {
    write_atomic_json(path, metadata)
}

fn write_atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), VoiceTurnError> {
    let parent = path.parent().ok_or_else(|| {
        VoiceTurnError::Metadata(std::io::Error::other("metadata path has no parent"))
    })?;
    std::fs::create_dir_all(parent).map_err(VoiceTurnError::Metadata)?;
    let temp = path.with_extension("tmp");
    let data = serde_json::to_vec(value).map_err(VoiceTurnError::Serialize)?;
    std::fs::write(&temp, data).map_err(VoiceTurnError::Metadata)?;
    std::fs::rename(temp, path).map_err(VoiceTurnError::Metadata)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VoiceRuntimeInfo {
    pub runtime_instance_id: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub capabilities: Value,
}

impl VoicePolicyCapabilities {
    pub(crate) fn from_runtime(runtime: &VoiceRuntimeInfo) -> Self {
        let voice = runtime
            .capabilities
            .get("capabilities")
            .and_then(|value| value.get("voice"))
            .unwrap_or(&runtime.capabilities);
        Self {
            provider_available: !runtime.runtime_instance_id.is_empty(),
            provider_is_paid: voice
                .get("provider_is_paid")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| !is_known_local_provider(runtime.provider.as_deref())),
            provider: runtime.provider.clone(),
            model: runtime.model.clone(),
            tools_supported: voice
                .get("tools_supported")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            estimated_cost_micros: voice.get("estimated_cost_micros").and_then(Value::as_u64),
        }
    }
}

fn is_known_local_provider(provider: Option<&str>) -> bool {
    provider
        .map(|provider| provider.to_ascii_lowercase().contains("ollama"))
        .unwrap_or(false)
}

/// Bootstrap de voz delegado al bootstrap local compartido. No contiene
/// manejo de sockets, locks ni arranque de hosts propios.
pub struct VoiceRuntimeBootstrap;

impl VoiceRuntimeBootstrap {
    pub async fn connect(mode: AcpTransportMode) -> Result<VoiceAcpClient, VoiceTurnError> {
        let transport = connect_local_client(mode)
            .await
            .map_err(VoiceTurnError::Bootstrap)?;
        Ok(VoiceAcpClient::from_transport(transport))
    }

    pub async fn connect_default() -> Result<VoiceAcpClient, VoiceTurnError> {
        Self::connect(AcpTransportMode::from_env().map_err(VoiceTurnError::Bootstrap)?).await
    }
}

pub struct VoiceAcpClient {
    client: AcpTuiClient,
    notifications: Mutex<mpsc::UnboundedReceiver<AcpNotification>>,
}

impl VoiceAcpClient {
    pub fn from_transport(transport: Arc<dyn AcpClientTransport>) -> Self {
        let (client, notifications) = AcpTuiClient::new(transport);
        client.spawn_pump();
        Self {
            client,
            notifications: Mutex::new(notifications),
        }
    }

    pub async fn reconnect(self, mode: AcpTransportMode) -> Result<Self, VoiceTurnError> {
        let _ = self.client.close().await;
        VoiceRuntimeBootstrap::connect(mode).await
    }

    pub async fn close(&self) -> Result<(), VoiceTurnError> {
        self.client.close().await?;
        Ok(())
    }

    pub async fn runtime_info(&self) -> Result<VoiceRuntimeInfo, VoiceTurnError> {
        let health = self.health().await?;
        if health.get("health").and_then(Value::as_str) != Some("ready") {
            return Err(VoiceTurnError::RuntimeUnavailable);
        }
        let identity = self.identity().await?;
        let capabilities = self.capabilities().await?;
        let runtime_instance_id = identity
            .get("runtime_instance_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or(VoiceTurnError::RuntimeUnavailable)?
            .to_string();
        Ok(VoiceRuntimeInfo {
            runtime_instance_id,
            provider: identity
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_string),
            model: identity
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            capabilities,
        })
    }

    pub async fn health(&self) -> Result<Value, VoiceTurnError> {
        Ok(self.client.request("health", json!({})).await?)
    }

    pub async fn identity(&self) -> Result<Value, VoiceTurnError> {
        Ok(self.client.request("runtime/identity", json!({})).await?)
    }

    pub async fn capabilities(&self) -> Result<Value, VoiceTurnError> {
        Ok(self
            .client
            .request("runtime/capabilities", json!({}))
            .await?)
    }

    pub async fn session_new(
        &self,
        workspace_id: &str,
    ) -> Result<VoiceSessionState, VoiceTurnError> {
        let response = self
            .client
            .request("session/new", json!({"cwd": workspace_id}))
            .await?;
        session_state_from_response(&response, workspace_id, None)
    }

    pub async fn session_load(
        &self,
        state: &VoiceSessionState,
    ) -> Result<VoiceSessionState, VoiceTurnError> {
        let response = self
            .client
            .request(
                "session/load",
                json!({
                    "sessionId": state.session_id,
                    "threadId": state.thread_id,
                    "cwd": state.workspace_id,
                }),
            )
            .await?;
        session_state_from_response(&response, &state.workspace_id, Some(state))
    }

    pub async fn ensure_session(
        &self,
        store: &VoiceSessionStore,
        workspace: &str,
    ) -> Result<VoiceSessionState, VoiceTurnError> {
        if let Some(metadata) = store.get() {
            return self.session_load(&metadata).await;
        }
        self.session_new(workspace).await
    }

    pub async fn prompt(
        &self,
        session_id: &str,
        envelope: &TaskEnvelopeV1,
    ) -> Result<Value, VoiceTurnError> {
        Ok(self
            .client
            .request(
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "message": {"role": "user", "content": envelope.user_input},
                    "taskEnvelope": envelope,
                }),
            )
            .await?)
    }

    pub async fn cancel(&self, session_id: &str) -> Result<(), VoiceTurnError> {
        self.client
            .notify("session/cancel", json!({"sessionId": session_id}))
            .await?;
        Ok(())
    }

    /// Responde un RequestPermission entrante en el mismo transport ACP que
    /// lo emitió. Nunca intenta ejecutar ni autorizar herramientas localmente.
    pub async fn resolve_permission(
        &self,
        request_id: RequestId,
        approved: bool,
    ) -> Result<(), VoiceTurnError> {
        use agent_client_protocol::schema::{
            RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome,
        };

        let response = if approved {
            RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new("allow_once"),
            ))
        } else {
            RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
        };
        let value = serde_json::to_value(response).map_err(VoiceTurnError::Serialize)?;
        self.client.send_response(request_id, Ok(value)).await?;
        Ok(())
    }

    /// El cambio de modelo es una operación del runtime ACP, no una escritura
    /// de settings desde el cliente de voz.
    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<(), VoiceTurnError> {
        self.client
            .request(
                "session/set_config_option",
                json!({"sessionId": session_id, "configId": config_id, "value": value}),
            )
            .await?;
        Ok(())
    }

    pub async fn subscribe(&self) -> Option<AcpNotification> {
        self.notifications.lock().await.recv().await
    }
}

fn session_state_from_response(
    response: &Value,
    workspace_id: &str,
    previous: Option<&VoiceSessionState>,
) -> Result<VoiceSessionState, VoiceTurnError> {
    let session_id = response
        .get("sessionId")
        .or_else(|| response.get("session_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| previous.map(|state| state.session_id.clone()))
        .ok_or(VoiceTurnError::Protocol("missing session id".into()))?;
    let thread_id = response
        .get("threadId")
        .or_else(|| response.get("thread_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| previous.map(|state| state.thread_id.clone()))
        .ok_or(VoiceTurnError::Protocol("missing ACP thread id".into()))?;
    Ok(VoiceSessionState {
        schema_version: 1,
        session_id,
        thread_id,
        workspace_id: workspace_id.to_string(),
        last_runtime_instance_id: previous
            .map(|state| state.last_runtime_instance_id.clone())
            .unwrap_or_default(),
        last_result_id: previous.and_then(|state| state.last_result_id.clone()),
        updated_at: now_epoch_ms(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceTurnState {
    Idle,
    Connecting,
    Ready,
    Routing,
    PolicyPending,
    ToolPermissionPending,
    CostConfirmationPending,
    Escalating,
    ModelSelected,
    Thinking,
    ToolStarted,
    ToolFinished,
    Streaming,
    BudgetWarning,
    BudgetExceeded,
    Completed,
    Cancelled,
    TimedOut,
    Failed,
}

impl Default for VoiceTurnState {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Clone, Default)]
pub struct HudModel {
    pub state: VoiceTurnState,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct VoiceHudBridge {
    model: HudModel,
}

impl VoiceHudBridge {
    pub fn apply_event(&mut self, status: &str) {
        match status {
            "connecting" => self.connecting(),
            "ready" => self.ready(),
            "routing" => self.routing(),
            "policy_pending" => self.policy_pending(),
            "tool_permission_pending" => self.pending_tool_permission(),
            "cost_confirmation_pending" => self.pending_cost_confirmation(),
            "escalated" => self.model.state = VoiceTurnState::Escalating,
            "model_selected" => self.model_selected(),
            "thinking" => self.thinking(),
            "tool_started" => self.tool_started(),
            "tool_finished" => self.tool_finished(),
            "streaming" => self.model.state = VoiceTurnState::Streaming,
            "budget_warning" => self.budget_warning(),
            "budget_exceeded" => self.budget_exceeded(),
            "completed" => self.model.state = VoiceTurnState::Completed,
            "cancelled" => self.cancelled(),
            "failed" => self.failed(),
            _ => {}
        }
    }

    pub fn connecting(&mut self) {
        self.model.state = VoiceTurnState::Connecting;
        self.model.message = "Conectando con Nexum.".into();
    }

    pub fn ready(&mut self) {
        self.model.state = VoiceTurnState::Ready;
        self.model.message = "Nexum listo.".into();
    }

    pub fn routing(&mut self) {
        self.model.state = VoiceTurnState::Routing;
        self.model.message = "Decidiendo la ruta.".into();
    }
    pub fn escalating(&mut self, provider: Option<String>, model: Option<String>) {
        self.model.state = VoiceTurnState::Escalating;
        self.model.provider = provider;
        self.model.model = model;
        self.model.message = "Resolviendo consulta compleja.".into();
    }

    pub fn policy_pending(&mut self) {
        self.model.state = VoiceTurnState::PolicyPending;
        self.model.message = "Esperando autorización de política.".into();
    }

    pub fn pending_tool_permission(&mut self) {
        self.model.state = VoiceTurnState::ToolPermissionPending;
        self.model.message = "Esperando permiso para usar una herramienta.".into();
    }

    pub fn pending_cost_confirmation(&mut self) {
        self.model.state = VoiceTurnState::CostConfirmationPending;
        self.model.message = "Esperando confirmación de costo.".into();
    }

    pub fn model_selected(&mut self) {
        self.model.state = VoiceTurnState::ModelSelected;
        self.model.message = "Modelo seleccionado.".into();
    }

    pub fn thinking(&mut self) {
        self.model.state = VoiceTurnState::Thinking;
        self.model.message = "Pensando.".into();
    }

    pub fn tool_started(&mut self) {
        self.model.state = VoiceTurnState::ToolStarted;
        self.model.message = "Ejecutando una herramienta.".into();
    }

    pub fn tool_finished(&mut self) {
        self.model.state = VoiceTurnState::ToolFinished;
        self.model.message = "Herramienta terminada.".into();
    }

    pub fn budget_warning(&mut self) {
        self.model.state = VoiceTurnState::BudgetWarning;
        self.model.message = "El pedido se acerca a su límite.".into();
    }

    pub fn budget_exceeded(&mut self) {
        self.model.state = VoiceTurnState::BudgetExceeded;
        self.model.message = "El pedido alcanzó su límite.".into();
    }

    pub fn streaming(&mut self, text: &str) {
        self.model.state = VoiceTurnState::Streaming;
        self.model.message = VoiceResultFormatter::format(text);
    }

    pub fn streaming_event(&mut self, text: &str, provider: Option<&str>, model: Option<&str>) {
        self.streaming(text);
        if let Some(provider) = provider {
            self.model.provider = Some(sanitize_label(provider));
        }
        if let Some(model) = model {
            self.model.model = Some(sanitize_label(model));
        }
    }

    pub fn completed(&mut self, text: &str) {
        self.model.state = VoiceTurnState::Completed;
        self.model.message = VoiceResultFormatter::format(text);
    }

    pub fn cancelled(&mut self) {
        self.model.state = VoiceTurnState::Cancelled;
        self.model.message = "Cancelé el pedido.".into();
    }

    pub fn timeout(&mut self) {
        self.model.state = VoiceTurnState::TimedOut;
        self.model.message = "El pedido demoró demasiado. Probá de nuevo.".into();
    }

    pub fn failed(&mut self) {
        self.model.state = VoiceTurnState::Failed;
        self.model.message = "No pude completar el pedido.".into();
    }

    pub fn state(&self) -> VoiceTurnState {
        self.model.state
    }

    pub fn message(&self) -> &str {
        &self.model.message
    }

    pub fn model(&self) -> &HudModel {
        &self.model
    }
}

pub struct VoiceResultFormatter;

impl VoiceResultFormatter {
    pub fn format(text: &str) -> String {
        let mut in_code = false;
        let output: Vec<_> = text
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with("```") {
                    in_code = !in_code;
                    return None;
                }
                if in_code || is_technical_line(trimmed) {
                    None
                } else {
                    (!trimmed.is_empty()).then_some(trimmed)
                }
            })
            .collect();
        output.join(" ")
    }
}

fn is_technical_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    line.contains('/')
        || line.contains('\\')
        || (line.starts_with('{') && line.ends_with('}'))
        || (line.starts_with('[') && line.ends_with(']'))
        || line.contains("{\"")
        || line.contains("\":")
        || lower.starts_with("panic")
        || lower.starts_with("thread '")
        || lower.starts_with("at ")
        || lower.starts_with("stack")
        || lower.starts_with("fn ")
        || lower.starts_with("let ")
        || lower.starts_with("use ")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VoiceTurnTrace {
    pub turn_id: String,
    pub envelope_id: String,
    pub session_id: String,
    pub thread_id: String,
    pub runtime_instance_id: String,
    pub route: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub budget_status: String,
    pub tool_calls: u32,
    pub evidence_ref_count: usize,
    pub result_status: String,
    pub duration: u64,
}

impl VoiceTurnTrace {
    fn escalated(
        runtime: &VoiceRuntimeInfo,
        state: &VoiceSessionState,
        envelope: &TaskEnvelopeV1,
        started: Instant,
        result_status: &str,
        budget_status: &str,
        tool_calls: u32,
    ) -> Self {
        Self {
            turn_id: uuid::Uuid::new_v4().to_string(),
            envelope_id: sanitize_label(&envelope.envelope_id),
            session_id: sanitize_label(&state.session_id),
            thread_id: sanitize_label(&state.thread_id),
            runtime_instance_id: sanitize_label(&runtime.runtime_instance_id),
            route: "escalated".into(),
            provider: runtime.provider.as_deref().map(sanitize_label),
            model: runtime.model.as_deref().map(sanitize_label),
            budget_status: budget_status.into(),
            tool_calls,
            evidence_ref_count: envelope.evidence_refs.len(),
            result_status: result_status.into(),
            duration: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        }
    }
}

fn sanitize_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        .take(120)
        .collect()
}

#[derive(Debug, Clone)]
pub struct VoiceTurnResult {
    pub full_ref: Option<String>,
    pub hud_text: String,
    pub speakable: String,
    pub warnings: Vec<String>,
    pub status: String,
    pub trace: VoiceTurnTrace,
}

pub struct VoiceTurnController {
    client: VoiceAcpClient,
    store: VoiceSessionStore,
    hud: VoiceHudBridge,
    preferences: VoicePreferences,
    preferences_store: Option<VoicePreferencesStore>,
    pending: PendingVoiceActions,
    pending_escalations: HashMap<String, (TaskEnvelopeV1, String)>,
    permission_requests: HashMap<String, RequestId>,
    pending_model_changes: HashMap<String, (String, String)>,
    policy_gate_enabled: bool,
}

impl VoiceTurnController {
    pub fn new(client: VoiceAcpClient, store: VoiceSessionStore, hud: VoiceHudBridge) -> Self {
        Self {
            client,
            store,
            hud,
            preferences: VoicePreferences {
                policy: VoiceEscalationPolicy::Smart,
                allow_tools: true,
                allow_paid_providers: true,
                ..Default::default()
            },
            preferences_store: None,
            pending: PendingVoiceActions::default(),
            pending_escalations: HashMap::new(),
            permission_requests: HashMap::new(),
            pending_model_changes: HashMap::new(),
            policy_gate_enabled: false,
        }
    }

    pub fn with_preferences(
        client: VoiceAcpClient,
        store: VoiceSessionStore,
        hud: VoiceHudBridge,
        preferences: VoicePreferences,
    ) -> Self {
        Self {
            client,
            store,
            hud,
            preferences,
            preferences_store: None,
            pending: PendingVoiceActions::default(),
            pending_escalations: HashMap::new(),
            permission_requests: HashMap::new(),
            pending_model_changes: HashMap::new(),
            policy_gate_enabled: true,
        }
    }

    pub fn with_preferences_store(
        client: VoiceAcpClient,
        store: VoiceSessionStore,
        hud: VoiceHudBridge,
        preferences_store: VoicePreferencesStore,
    ) -> Self {
        let preferences = preferences_store.load();
        let mut controller = Self::with_preferences(client, store, hud, preferences);
        controller.preferences_store = Some(preferences_store);
        controller
    }

    pub fn hud(&self) -> &VoiceHudBridge {
        &self.hud
    }

    pub fn pending_actions(&self) -> Vec<&PendingVoiceAction> {
        self.pending.active()
    }

    pub async fn next_notification(&self) -> Option<AcpNotification> {
        self.client.subscribe().await
    }

    /// Convierte el request ACP entrante en una acción pendiente; no responde
    /// hasta que la misma acción sea confirmada o rechazada explícitamente.
    pub fn register_permission_request(&mut self, request_id: RequestId, params: Value) -> String {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let request_label = format!("{request_id:?}");
        let id = self.add_pending(
            session_id,
            Some(request_label),
            PendingVoiceActionKind::ToolPermission,
        );
        self.permission_requests.insert(id.clone(), request_id);
        self.hud.pending_tool_permission();
        id
    }

    pub async fn resolve_pending_action(
        &mut self,
        action_id: &str,
        approved: bool,
    ) -> Result<(), VoiceTurnError> {
        let status = if approved {
            PendingVoiceActionStatus::Approved
        } else {
            PendingVoiceActionStatus::Rejected
        };
        let action = self
            .pending
            .resolve_action_by_id(action_id, status, now_epoch_ms())
            .ok_or_else(|| VoiceTurnError::PendingActionNotFound(action_id.into()))?;
        if let Some(request_id) = self.permission_requests.remove(action_id) {
            self.client.resolve_permission(request_id, approved).await?;
        }
        if let Some((session_id, model)) = self.pending_model_changes.remove(action_id) {
            if approved {
                self.client
                    .set_config_option(&session_id, "model", &model)
                    .await?;
                self.hud.model_selected();
            }
        }
        if action.kind == PendingVoiceActionKind::Escalation {
            let (envelope, workspace) = self
                .pending_escalations
                .remove(action_id)
                .ok_or_else(|| VoiceTurnError::PendingActionNotFound(action_id.into()))?;
            if approved {
                self.execute_escalation(envelope, &workspace).await?;
            }
        }
        Ok(())
    }

    pub async fn cancel(&mut self) -> Result<(), VoiceTurnError> {
        let state = self.store.get().ok_or(VoiceTurnError::NoSession)?;
        self.client.cancel(&state.session_id).await?;
        self.hud.cancelled();
        Ok(())
    }

    pub async fn execute(
        &mut self,
        decision: VoiceRouteDecision,
        workspace: &str,
    ) -> Result<VoiceTurnResult, VoiceTurnError> {
        self.hud.routing();
        let decision = decision.validate()?;
        let decision = if self.policy_gate_enabled {
            match &decision {
                VoiceRouteDecision::Escalate { envelope, .. } => {
                    let runtime = self.client.runtime_info().await?;
                    VoicePolicyGate::route(
                        &self.preferences,
                        envelope.clone(),
                        &VoicePolicyCapabilities::from_runtime(&runtime),
                    )
                }
                _ => decision,
            }
        } else {
            decision
        };
        match decision {
            VoiceRouteDecision::Local { response, .. } => {
                self.hud.completed(&response.text_speakable);
                Ok(VoiceTurnResult {
                    full_ref: None,
                    hud_text: self.hud.message().to_string(),
                    speakable: response.text_speakable,
                    warnings: Vec::new(),
                    status: "completed".into(),
                    trace: VoiceTurnTrace {
                        route: "local".into(),
                        ..Default::default()
                    },
                })
            }
            VoiceRouteDecision::AskForEscalation {
                envelope,
                provider,
                model,
                reason,
            } => {
                let action = self.add_pending(
                    &envelope.session_id,
                    None,
                    PendingVoiceActionKind::Escalation,
                );
                self.pending_escalations
                    .insert(action.clone(), (envelope, workspace.to_string()));
                self.hud.policy_pending();
                self.hud.model.provider = Some(sanitize_label(&provider));
                self.hud.model.model = Some(sanitize_label(&model));
                self.hud.model.message = VoiceResultFormatter::format(&reason);
                Ok(VoiceTurnResult {
                    full_ref: Some(action),
                    hud_text: self.hud.message().to_string(),
                    speakable: VoiceResultFormatter::format(&reason),
                    warnings: vec!["Se requiere confirmación antes de escalar.".into()],
                    status: "awaiting_escalation_confirmation".into(),
                    trace: VoiceTurnTrace {
                        route: "ask_for_escalation".into(),
                        ..Default::default()
                    },
                })
            }
            VoiceRouteDecision::ToolPermissionRequired {
                envelope,
                tools,
                reason,
            } => {
                let action = self.add_pending(
                    &envelope.session_id,
                    None,
                    PendingVoiceActionKind::ToolPermission,
                );
                self.hud.pending_tool_permission();
                Ok(VoiceTurnResult {
                    full_ref: Some(action),
                    hud_text: self.hud.message().to_string(),
                    speakable: format!("Necesito permiso para usar: {}.", tools.join(", ")),
                    warnings: vec![reason],
                    status: "awaiting_tool_permission".into(),
                    trace: VoiceTurnTrace {
                        route: "tool_permission_required".into(),
                        ..Default::default()
                    },
                })
            }
            VoiceRouteDecision::CostConfirmationRequired {
                envelope,
                provider,
                model,
                estimated_cost_micros,
                reason,
            } => {
                let action = self.add_pending(
                    &envelope.session_id,
                    None,
                    PendingVoiceActionKind::CostConfirmation,
                );
                self.hud.pending_cost_confirmation();
                Ok(VoiceTurnResult {
                    full_ref: Some(action),
                    hud_text: self.hud.message().to_string(),
                    speakable: format!(
                        "Necesito confirmar el costo estimado de {estimated_cost_micros} micros para {provider} {model}."
                    ),
                    warnings: vec![reason],
                    status: "awaiting_cost_confirmation".into(),
                    trace: VoiceTurnTrace {
                        route: "cost_confirmation_required".into(),
                        ..Default::default()
                    },
                })
            }
            VoiceRouteDecision::ModelDirective { directive } => {
                let (message, status) = match directive {
                    super::model_directive::ModelDirective::SwitchTo(model) => {
                        let state = self.client.ensure_session(&self.store, workspace).await?;
                        self.store.replace(state.clone())?;
                        if self.policy_gate_enabled
                            && self.preferences.policy == VoiceEscalationPolicy::Local
                        {
                            return Err(VoiceTurnError::Blocked(
                                "La política de voz permite solo resolución local.".into(),
                            ));
                        }
                        if self.policy_gate_enabled
                            && self.preferences.policy == VoiceEscalationPolicy::Ask
                        {
                            let action = self.add_pending(
                                &state.session_id,
                                None,
                                PendingVoiceActionKind::Model,
                            );
                            self.pending_model_changes
                                .insert(action.clone(), (state.session_id, model));
                            self.hud.policy_pending();
                            return Ok(VoiceTurnResult {
                                full_ref: Some(action),
                                hud_text: self.hud.message().to_string(),
                                speakable: "Necesito confirmar el cambio de modelo.".into(),
                                warnings: vec![
                                    "La política de voz requiere confirmación de modelo.".into(),
                                ],
                                status: "awaiting_model_confirmation".into(),
                                trace: VoiceTurnTrace {
                                    route: "model_pending".into(),
                                    ..Default::default()
                                },
                            });
                        }
                        let runtime = self.change_model(&state, &model).await?;
                        (
                            format!(
                                "Modelo solicitado por ACP: {}.",
                                runtime.model.as_deref().unwrap_or(&model)
                            ),
                            "model_changed",
                        )
                    }
                    super::model_directive::ModelDirective::ShowCurrent => {
                        let runtime = self.client.runtime_info().await?;
                        (
                            format!(
                                "Modelo actual: {}.",
                                runtime.model.as_deref().unwrap_or("no informado por ACP")
                            ),
                            "model_current",
                        )
                    }
                    super::model_directive::ModelDirective::PersistDefault => {
                        let runtime = self.client.runtime_info().await?;
                        let model = runtime.model.ok_or_else(|| {
                            VoiceTurnError::Protocol("ACP did not report the current model".into())
                        })?;
                        self.preferences.default_model = Some(model.clone());
                        self.save_preferences()?;
                        (
                            format!("Modelo predeterminado de voz: {model}."),
                            "model_default_persisted",
                        )
                    }
                    super::model_directive::ModelDirective::PreviousModel => {
                        let model = self.preferences.previous_model.clone().ok_or_else(|| {
                            VoiceTurnError::Protocol("No hay un modelo anterior guardado.".into())
                        })?;
                        let state = self.client.ensure_session(&self.store, workspace).await?;
                        self.store.replace(state.clone())?;
                        let runtime = self.change_model(&state, &model).await?;
                        (
                            format!(
                                "Volví al modelo anterior: {}.",
                                runtime.model.as_deref().unwrap_or(&model)
                            ),
                            "model_changed",
                        )
                    }
                };
                Ok(VoiceTurnResult {
                    full_ref: None,
                    hud_text: message.clone(),
                    speakable: message,
                    warnings: Vec::new(),
                    status: status.into(),
                    trace: VoiceTurnTrace {
                        route: "model_directive".into(),
                        ..Default::default()
                    },
                })
            }
            VoiceRouteDecision::NeedMoreContext { missing_fields } => {
                let action = self.add_pending("", None, PendingVoiceActionKind::Context);
                self.hud.policy_pending();
                Ok(VoiceTurnResult {
                    full_ref: Some(action),
                    hud_text: self.hud.message().to_string(),
                    speakable: format!("Necesito más contexto: {}.", missing_fields.join(", ")),
                    warnings: missing_fields,
                    status: "awaiting_context".into(),
                    trace: VoiceTurnTrace {
                        route: "context_required".into(),
                        ..Default::default()
                    },
                })
            }
            VoiceRouteDecision::Blocked { reason } => {
                self.hud.failed();
                Err(VoiceTurnError::Blocked(reason))
            }
            VoiceRouteDecision::Failed { error } => {
                self.hud.failed();
                Err(VoiceTurnError::Failed(error))
            }
            VoiceRouteDecision::Escalate { envelope, .. } => {
                self.execute_escalation(envelope, workspace).await
            }
        }
    }

    fn save_preferences(&self) -> Result<(), VoiceTurnError> {
        if let Some(store) = &self.preferences_store {
            store.save(&self.preferences)?;
        }
        Ok(())
    }

    async fn change_model(
        &mut self,
        state: &VoiceSessionState,
        model: &str,
    ) -> Result<VoiceRuntimeInfo, VoiceTurnError> {
        let previous = self.client.runtime_info().await?.model;
        self.client
            .set_config_option(&state.session_id, "model", model)
            .await?;
        let runtime = self.client.runtime_info().await?;
        if previous.as_deref() != runtime.model.as_deref() {
            self.preferences.previous_model = previous;
        }
        self.save_preferences()?;
        self.hud.model_selected();
        Ok(runtime)
    }

    async fn execute_escalation(
        &mut self,
        mut envelope: TaskEnvelopeV1,
        workspace: &str,
    ) -> Result<VoiceTurnResult, VoiceTurnError> {
        let started = Instant::now();
        self.hud.connecting();
        let runtime = self.client.runtime_info().await?;
        self.hud.ready();
        self.hud
            .escalating(runtime.provider.clone(), runtime.model.clone());
        self.hud.model_selected();
        let mut state = self.client.ensure_session(&self.store, workspace).await?;
        envelope.session_id = state.session_id.clone();
        envelope.thread_id = state.thread_id.clone();
        state.last_runtime_instance_id = runtime.runtime_instance_id.clone();
        self.store.replace(state.clone())?;
        let (text, budget_status, tool_calls, result_status) =
            match tokio::time::timeout(wait_budget(&envelope), async {
                let prompt_result = self.client.prompt(&state.session_id, &envelope).await?;
                state.last_result_id = prompt_result
                    .get("resultId")
                    .or_else(|| prompt_result.get("result_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.store.replace(state.clone())?;
                self.wait_for_result(&state.session_id, &envelope).await
            })
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    self.hud.timeout();
                    return Err(VoiceTurnError::TimedOut);
                }
            };
        self.hud.completed(&text);
        Ok(VoiceTurnResult {
            full_ref: state.last_result_id.clone(),
            hud_text: self.hud.message().to_string(),
            speakable: VoiceResultFormatter::format(&text),
            warnings: Vec::new(),
            status: result_status.clone(),
            trace: VoiceTurnTrace::escalated(
                &runtime,
                &state,
                &envelope,
                started,
                &result_status,
                &budget_status,
                tool_calls,
            ),
        })
    }

    fn add_pending(
        &mut self,
        session_id: &str,
        request_id: Option<String>,
        kind: PendingVoiceActionKind,
    ) -> String {
        let action = PendingVoiceAction::new(
            uuid::Uuid::new_v4().to_string(),
            session_id,
            request_id,
            kind,
            now_epoch_ms(),
            Duration::from_secs(120).as_millis() as u64,
        );
        let id = action.id.clone();
        self.pending.add(action);
        id
    }

    async fn wait_for_result(
        &mut self,
        session_id: &str,
        envelope: &TaskEnvelopeV1,
    ) -> Result<(String, String, u32, String), VoiceTurnError> {
        let wait = async {
            let mut text = String::new();
            let mut tool_calls: u32 = 0;
            let mut budget_status = "ok".to_string();
            while let Some(notification) = self.client.subscribe().await {
                match notification {
                    AcpNotification::RequestPermission { id, params } => {
                        self.register_permission_request(id, params);
                    }
                    AcpNotification::AgentEvent {
                        session_id: received,
                        event,
                    } if received == session_id => match event {
                        nexum_agent::agent::events::AgentEvent::AiReasoning(_) => {
                            self.hud.thinking();
                        }
                        nexum_agent::agent::events::AgentEvent::TextChunk { chunk, .. } => {
                            text.push_str(&chunk);
                            self.hud.streaming_event(&text, None, None);
                        }
                        nexum_agent::agent::events::AgentEvent::ToolStart { .. } => {
                            tool_calls = tool_calls.saturating_add(1);
                            self.hud.tool_started();
                        }
                        nexum_agent::agent::events::AgentEvent::ToolEnd { .. } => {
                            self.hud.tool_finished();
                        }
                        nexum_agent::agent::events::AgentEvent::AgentExecutionFailed {
                            message,
                        } => {
                            if message.to_ascii_lowercase().contains("interrupted") {
                                self.hud.cancelled();
                                return Err(VoiceTurnError::Cancelled);
                            }
                            self.hud.failed();
                            return Err(VoiceTurnError::RuntimeUnavailable);
                        }
                        _ => {}
                    },
                    AcpNotification::SessionUpdate {
                        session_id: received,
                        params,
                    } if received == session_id => {
                        let update_type = params
                            .get("update")
                            .and_then(|update| update.get("sessionUpdate"))
                            .and_then(Value::as_str);
                        match update_type {
                            Some("agent_message_chunk") => {
                                if let Some(chunk) = params
                                    .get("update")
                                    .and_then(|update| update.get("content"))
                                    .and_then(|content| content.get("text"))
                                    .and_then(Value::as_str)
                                {
                                    text.push_str(chunk);
                                    self.hud.streaming_event(&text, None, None);
                                }
                            }
                            Some("agent_thought_chunk") => self.hud.thinking(),
                            Some("tool_call") => {
                                tool_calls = tool_calls.saturating_add(1);
                                self.hud.tool_started();
                            }
                            Some("tool_call_update") => self.hud.tool_finished(),
                            _ => {}
                        }
                        if let Some(chunk) = params.get("text").and_then(Value::as_str) {
                            text.push_str(chunk);
                            self.hud.streaming_event(
                                &text,
                                params.get("provider").and_then(Value::as_str),
                                params.get("model").and_then(Value::as_str),
                            );
                        }
                        let status = params.get("status").and_then(Value::as_str);
                        if let Some(status) = status {
                            self.hud.apply_event(status);
                        }
                        match status {
                            Some("tool_started") => {
                                tool_calls = tool_calls.saturating_add(1);
                            }
                            Some("budget_warning") => {
                                budget_status = "warning".into();
                                self.hud.budget_warning();
                            }
                            Some("budget_exceeded") => {
                                self.hud.budget_exceeded();
                                return Err(VoiceTurnError::TimedOut);
                            }
                            Some("cancelled") => {
                                self.hud.cancelled();
                                return Err(VoiceTurnError::Cancelled);
                            }
                            Some("failed") => {
                                self.hud.failed();
                                return Err(VoiceTurnError::RuntimeUnavailable);
                            }
                            _ => {}
                        }
                        if budget_cancelled(&params) {
                            self.hud.cancelled();
                            return Err(VoiceTurnError::Cancelled);
                        }
                        if budget_timed_out(&params) {
                            self.hud.timeout();
                            return Err(VoiceTurnError::TimedOut);
                        }
                    }
                    AcpNotification::AgentDone {
                        session_id: received,
                    } if received == session_id => {
                        return Ok((text, budget_status, tool_calls, "completed".into()));
                    }
                    _ => {}
                }
            }
            Err(VoiceTurnError::RuntimeUnavailable)
        };
        match tokio::time::timeout(wait_budget(envelope), wait).await {
            Ok(result) => result,
            Err(_) => {
                self.hud.timeout();
                Err(VoiceTurnError::TimedOut)
            }
        }
    }
}

fn wait_budget(envelope: &TaskEnvelopeV1) -> Duration {
    envelope
        .execution_budget
        .wall_time_ms
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TURN_WAIT)
        .min(MAX_TURN_WAIT)
}

fn budget_cancelled(params: &Value) -> bool {
    params
        .get("budget")
        .and_then(|budget| budget.get("state"))
        .and_then(Value::as_str)
        == Some("cancelled")
}

fn budget_timed_out(params: &Value) -> bool {
    params
        .get("budget")
        .and_then(|budget| budget.get("state"))
        .and_then(Value::as_str)
        == Some("exceeded")
}

#[derive(Debug, thiserror::Error)]
pub enum VoiceTurnError {
    #[error("ACP runtime is unavailable")]
    RuntimeUnavailable,
    #[error("voice route is invalid")]
    InvalidRoute,
    #[error("unsupported voice preferences schema version: {0}")]
    InvalidPreferencesVersion(u32),
    #[error("voice needs more context: {0}")]
    NeedMoreContext(String),
    #[error("voice route is blocked: {0}")]
    Blocked(String),
    #[error("voice route failed: {0}")]
    Failed(String),
    #[error("voice session is unavailable")]
    NoSession,
    #[error("voice turn was cancelled")]
    Cancelled,
    #[error("voice turn timed out")]
    TimedOut,
    #[error("pending voice action was not found: {0}")]
    PendingActionNotFound(String),
    #[error("ACP protocol error: {0}")]
    Protocol(String),
    #[error("ACP error: {0}")]
    Acp(#[from] AcpError),
    #[error("ACP bootstrap error: {0}")]
    Bootstrap(anyhow::Error),
    #[error("metadata I/O error: {0}")]
    Metadata(std::io::Error),
    #[error("metadata serialization error: {0}")]
    Serialize(serde_json::Error),
}
