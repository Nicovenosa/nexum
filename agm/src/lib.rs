pub mod commands;
pub mod error;
pub mod filter;
pub mod types;
pub use error::{AgmError, Result};

pub mod adapter;
pub mod config;
pub mod fs_util;
pub mod git;
pub mod installer;
pub mod registry;
pub mod resolver;
pub mod store;

#[cfg(test)]
mod config_test;

#[cfg(test)]
mod types_test;

#[cfg(test)]
mod filter_test;

#[cfg(test)]
mod store_test;

#[cfg(test)]
mod self_update_test;
