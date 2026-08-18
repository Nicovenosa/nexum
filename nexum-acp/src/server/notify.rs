//! ACP Notification dispatch — handles incoming notifications and pushes
//! session update notifications. Extracted from original acp_server.rs (2026-05-20 split).

use std::collections::HashMap;

use agent_client_protocol::schema::{AvailableCommandsUpdate, SessionUpdate};
use nexum_acp::dispatch::config_update;
use nexum_middlewares::skills::SkillMetadata;
use serde_json::Value;
use tracing::{debug, info};

use super::{AcpServerConfig, SessionState};
use crate::provider::LlmProvider;

// ── Notification dispatch ────────────────────────────────────────────────────

pub(crate) fn handle_notification(
    method: &str,
    params: &Value,
    sessions: &HashMap<String, SessionState>,
    cfg: &AcpServerConfig,
) {
    match method {
        "session/cancel" => {
            let session_id = extract_session_id(params, "");
            if let Some(state) = sessions.get(session_id) {
                if let Some(ref token) = state.cancel_token {
                    token.cancel();
                    info!(session_id = %session_id, "Cancel requested");
                }
            }
        }
        "session/config_update" => {
            // Two formats:
            // 1. {"config": NexumConfig} — full config replace (from update_config)
            // 2. {"configId": "model"/"provider", "value": "..."} — partial (from set_config_option)
            if let Some(config_val) = params.get("config") {
                let new_cfg: crate::provider::NexumConfig = match serde_json::from_value(
                    config_val.clone(),
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, "config_update notification: invalid config");
                        return;
                    }
                };
                if let Err(error) = crate::provider::routes::enforce_runtime_selection(
                    &new_cfg.config.active_provider_id,
                    &new_cfg.config.active_alias,
                ) {
                    tracing::warn!(
                        error = %error,
                        "config_update notification: rejected provider/model route"
                    );
                    return;
                }
                let Some(new_provider) = LlmProvider::from_config(&new_cfg) else {
                    tracing::warn!(
                        provider_id = %new_cfg.config.active_provider_id,
                        model_id = %new_cfg.config.active_alias,
                        "PROVIDER_ACTIVATION_FAILED: full config has no executable credentials"
                    );
                    return;
                };
                tracing::info!(
                    active_provider = %new_cfg.config.active_provider_id,
                    provider_count = new_cfg.config.providers.len(),
                    "config_update notification: full config replace"
                );
                *cfg.nexum_config.write() = new_cfg.clone();
                // El provider ya viene validado: sin Option, no hay rama None
                // silenciosa. La identidad de runtime se actualiza igual, que es
                // lo que hace que el footer y la traza digan el provider real.
                cfg.runtime.identity.write().update_provider(&new_provider);
                *cfg.provider.write() = new_provider;
            } else if let (Some(config_id), Some(value)) = (
                params.get("configId").and_then(|v| v.as_str()),
                params.get("value").and_then(|v| v.as_str()),
            ) {
                match config_id {
                    "model" => {
                        let active_provider_id =
                            cfg.nexum_config.read().config.active_provider_id.clone();
                        if let Err(error) =
                            crate::provider::routes::enforce_runtime_selection(
                                &active_provider_id,
                                value,
                            )
                        {
                            tracing::warn!(
                                error = %error,
                                "config_update notification: rejected provider/model route"
                            );
                            return;
                        }
                        let mut c = cfg.nexum_config.write();
                        c.config.active_alias = value.to_string();
                        drop(c);
                        let new_provider = {
                            let c = cfg.nexum_config.read();
                            LlmProvider::from_config_for_alias(&c, value)
                        };
                        if let Some(p) = new_provider {
                            tracing::info!(alias = %value, "config_update notification: model changed");
                            cfg.runtime.identity.write().update_provider(&p);
                            *cfg.provider.write() = p;
                        }
                    }
                    other => {
                        tracing::debug!(config_id = %other, "config_update notification: unhandled configId");
                    }
                }
            } else {
                tracing::debug!("config_update notification: missing config/configId");
            }
            // No sessions to invalidate — pool will be built fresh on next session/new
        }
        _ => {
            debug!(method = %method, "Unhandled notification");
        }
    }
}

// ── Notification helpers ───────────────────────────────────────────────────────

/// Extract `sessionId` from JSON-RPC params, returning `default_value` if absent.
pub(crate) fn extract_session_id<'a>(params: &'a Value, default_value: &'a str) -> &'a str {
    params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(|v| v.as_str())
        .unwrap_or(default_value)
}

/// Build the current set of config options and push a `ConfigOptionUpdate` notification.
pub(crate) async fn send_config_option_update(
    transport: &dyn nexum_acp::transport::AcpTransport,
    session_id: &str,
    cfg: &AcpServerConfig,
) {
    if session_id.is_empty() {
        return;
    }
    let update = {
        let c = cfg.nexum_config.read();
        let p = cfg.provider.read();
        SessionUpdate::ConfigOptionUpdate(config_update::make_config_option_update(
            &c,
            &p,
            cfg.permission_mode.load(),
        ))
    };
    let update_value = match serde_json::to_value(&update) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize ConfigOptionUpdate");
            return;
        }
    };
    let payload = serde_json::json!({
        "sessionId": session_id,
        "update": update_value,
    });
    let _ = transport.send_notification("session/update", payload).await;
}

/// Push an `AvailableCommandsUpdate` notification for the given session.
pub(crate) async fn send_available_commands_update(
    transport: &dyn nexum_acp::transport::AcpTransport,
    session_id: &str,
    skills: &[SkillMetadata],
) {
    if session_id.is_empty() {
        return;
    }
    let commands = nexum_acp::dispatch::build_available_commands(skills);
    let update = SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(commands));
    let update_value = match serde_json::to_value(&update) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize AvailableCommandsUpdate");
            return;
        }
    };
    // Use {"update": ..., "sessionId": ...} format — same as TransportEventSink —
    // so that handle_session_update_peri on the TUI side can parse via params.get("update").
    let payload = serde_json::json!({
        "sessionId": session_id,
        "update": update_value,
    });
    let _ = transport.send_notification("session/update", payload).await;
}

/// Push a `SessionInfoUpdate` notification after prompt/compact completes.
pub(crate) async fn send_session_info_update(
    transport: &dyn nexum_acp::transport::AcpTransport,
    session_id: &str,
) {
    use agent_client_protocol::schema::SessionInfoUpdate;
    let info = SessionInfoUpdate::new().updated_at(chrono::Utc::now().to_rfc3339());
    let update = SessionUpdate::SessionInfoUpdate(info);
    let update_value = match serde_json::to_value(&update) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize SessionInfoUpdate");
            return;
        }
    };
    let payload = serde_json::json!({
        "sessionId": session_id,
        "update": update_value,
    });
    let _ = transport.send_notification("session/update", payload).await;
}
