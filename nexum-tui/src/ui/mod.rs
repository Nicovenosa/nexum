pub mod composer;
pub mod demo_mode;
#[cfg(any(test, feature = "headless"))]
pub mod headless;
pub mod main_ui;
pub mod markdown;
pub mod message_render;
pub mod message_view;
pub mod nexum_background;
pub mod nexum_mascot;
pub mod nexum_motion;
pub mod render_thread;
pub mod secret_redact;
pub mod theme;
pub mod tips;
pub mod welcome;

pub(crate) fn display_provider_name(provider_name: &str) -> String {
    let ollama_profile = std::env::var("NEXUM_PROVIDER")
        .map(|v| v.eq_ignore_ascii_case("ollama"))
        .unwrap_or(false)
        || std::env::var("NEXUM_PROVIDER_PROFILE")
            .map(|v| v.eq_ignore_ascii_case("ollama-local"))
            .unwrap_or(false);

    if ollama_profile && provider_name.eq_ignore_ascii_case("openai") {
        "Ollama Local".to_string()
    } else {
        provider_name.to_string()
    }
}
