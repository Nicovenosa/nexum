//! Shared ConfigOptionUpdate construction for TUI/Stdio paths.
//!
//! Both the TUI notify layer and the Stdio handler layer need to build
//! `ConfigOptionUpdate` values from the same trio of `(NexumConfig, LlmProvider, PermissionMode)`.
//! This module centralises that construction to avoid duplication.

use agent_client_protocol::schema::{ConfigOptionUpdate, SessionConfigOption};
use nexum_middlewares::prelude::PermissionMode;

use crate::provider::{LlmProvider, NexumConfig};
use crate::session::state_builders::build_config_options;

/// Build config options list from current config state.
pub fn make_config_options(
    nexum_config: &NexumConfig,
    provider: &LlmProvider,
    permission_mode: PermissionMode,
) -> Vec<SessionConfigOption> {
    build_config_options(nexum_config, provider, permission_mode)
}

/// Build a [`ConfigOptionUpdate`] from current config state.
pub fn make_config_option_update(
    nexum_config: &NexumConfig,
    provider: &LlmProvider,
    permission_mode: PermissionMode,
) -> ConfigOptionUpdate {
    let config_options = make_config_options(nexum_config, provider, permission_mode);
    ConfigOptionUpdate::new(config_options)
}
