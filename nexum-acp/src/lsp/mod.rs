//! LSP middleware integration.
//!
//! Re-exports `nexum_lsp` types and provides integration with
//! `nexum_middlewares::LspMiddleware` for the agent builder.
//!
//! LSP servers are configured in `AcpAgentConfig::lsp_servers`
//! and automatically registered when non-empty.

pub use nexum_lsp::config::LspServerConfig;
