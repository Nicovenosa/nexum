//! TUI interface for Rust Agent - interactive terminal playground

#![allow(
    clippy::if_same_then_else,
    clippy::needless_range_loop,
    clippy::reversed_empty_ranges,
    clippy::let_underscore_future,
    clippy::question_mark,
    clippy::collapsible_else_if,
    clippy::ptr_arg,
    clippy::infallible_try_from
)]

pub mod acp_client;
pub mod acp_server;
pub mod alloc_config;
pub mod app;
pub mod command;
pub mod doctor;
pub mod local_action;
pub mod config;
pub mod event;
pub mod hormiguero;
pub mod memory_gateway;
pub mod planning;
pub mod i18n;
pub mod layout;
pub mod sync;
pub mod thread;
pub mod ui;
pub mod update;
pub mod voice;

#[cfg(test)]
mod update_test;
#[cfg(test)]
mod alloc_config_test;
#[cfg(test)]
mod cron_architecture_test;
#[cfg(test)]
mod windows_installer_test;
#[cfg(test)]
mod notice_test;
#[cfg(test)]
mod layout_execution_test;
