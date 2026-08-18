//! nexum-acp — ACP Agent Service Layer
//!
//! Provides session management, agent construction, middleware chain assembly,
//! transport abstraction (mpsc/stdio), event mapping, HITL/AskUser broker, and
//! Langfuse tracing. Serves both TUI (via in-memory transport) and IDE (via stdio
//! transport) frontends.

extern crate self as nexum_acp;

pub mod agent;
pub mod broker;
pub mod cron;
pub mod dispatch;
pub mod event;
pub mod flow;
pub use nexum_agent::turn_log;
pub mod hooks;
pub mod langfuse;
pub mod lsp;
pub mod prompt;
pub mod provider;
pub mod runtime;
pub mod server;
pub mod session;
pub mod task;
pub mod transport;
