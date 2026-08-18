// Re-export LlmProvider from nexum-acp (single source of truth)
pub use nexum_acp::provider::LlmProvider;

#[cfg(test)]
#[path = "provider_test.rs"]
mod tests;
