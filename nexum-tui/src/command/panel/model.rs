use crate::{
    app::{agent, model_panel::{ModelChoice, ModelPanel}, App, MessageViewModel},
    command::Command,
    config::NexumConfig,
};

fn resolve_unique_model_choice(choices: &[ModelChoice], query: &str) -> Option<ModelChoice> {
    let query = query.trim().to_lowercase();
    let mut matches = choices.iter().filter(|choice| {
        query == choice.key.to_lowercase() || query == choice.label.to_lowercase()
    });
    let choice = matches.next()?.clone();
    matches.next().is_none().then_some(choice)
}

fn apply_model_choice(cfg: &mut NexumConfig, choice: &ModelChoice) {
    cfg.config.active_provider_id = choice.provider_id.clone();
    cfg.config.active_alias = choice.key.clone();
}

pub struct ModelCommand;

impl Command for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }

    fn aliases(&self) -> Vec<&str> {
        vec!["modelo"]
    }

    fn description(&self, _lc: &crate::i18n::LcRegistry) -> String {
        _lc.tr("command-model-description")
    }

    fn execute(&self, app: &mut App, args: &str) {
        let alias = args.trim().to_lowercase();
        if alias.is_empty() {
            app.open_model_panel();
            return;
        }

        let selected_choice = {
            let cfg = app.services.nexum_config.read();
            let panel = ModelPanel::from_config(&cfg);
            let choices: Vec<ModelChoice> = (0..panel.model_choice_count())
                .filter_map(|row| panel.model_choice(row).cloned())
                .collect();
            resolve_unique_model_choice(&choices, &alias)
        };

        match selected_choice {
            Some(choice) => {
                let cfg_arc = app.services.nexum_config.clone();
                let mut cfg = cfg_arc.write();
                apply_model_choice(&mut cfg, &choice);
                if let Err(e) = App::save_config(&cfg, app.services.config_path_override.as_deref())
                {
                    app.session_mgr.current_mut().messages.view_messages.push(
                        MessageViewModel::system(app.services.lc.tr_args(
                            "config-save-failed",
                            &[("error".into(), e.to_string().into())],
                        )),
                    );
                }
                if let Some(p) = agent::LlmProvider::from_config(&cfg) {
                    app.services.provider_name = p.display_name().to_string();
                    app.services.model_name = p.model_name().to_string();
                }
                if let Some(ref acp_client) = app.acp_client {
                    let acp = acp_client.clone();
                    let selected_config = cfg.clone();
                    tokio::spawn(async move {
                        let _ = acp.update_config(&selected_config).await;
                    });
                }
            }
            _ => {
                app.open_model_panel();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::RwLock;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        app::model_panel::ModelChoice,
        config::{AppConfig, NexumConfig, ProviderConfig, ThinkingConfig},
    };

    fn make_openai_config(active_alias: &str) -> NexumConfig {
        NexumConfig {
            schema: None,
            config: AppConfig {
                active_alias: active_alias.to_string(),
                active_provider_id: "openai-compatible".to_string(),
                providers: vec![ProviderConfig {
                    id: "openai-compatible".to_string(),
                    provider_type: "openai".to_string(),
                    api_key: "test-key".to_string(),
                    name: Some("OpenAI".to_string()),
                    ..Default::default()
                }],
                thinking: Some(ThinkingConfig {
                    enabled: true,
                    budget_tokens: 8000,
                    effort: "medium".to_string(),
                    max_tokens: 32000,
                }),
                ..Default::default()
            },
        }
    }

    async fn make_app_with_config(cfg: NexumConfig) -> App {
        let (mut app, _) = App::new_headless(80, 24).await;
        let config_file = tempdir().unwrap().keep().join("settings.json");
        app.services.config_path_override = Some(config_file);
        app.services.nexum_config = Arc::new(RwLock::new(cfg));
        app
    }

    #[tokio::test]
    async fn test_modelo_rejects_model_arg_absent_from_catalog() {
        let mut app = make_app_with_config(make_openai_config("opus")).await;
        ModelCommand.execute(&mut app, "deepseek-v4-flash");
        assert_eq!(
            app.services.nexum_config.read().config.active_alias,
            "opus"
        );
    }

    #[tokio::test]
    async fn test_modelo_rejects_claude_alias_for_openai_provider() {
        let mut app = make_app_with_config(make_openai_config("deepseek-v4-flash")).await;
        ModelCommand.execute(&mut app, "opus");
        assert_eq!(
            app.services.nexum_config.read().config.active_alias,
            "deepseek-v4-flash"
        );
    }

    #[test]
    fn test_model_argument_requires_one_unique_catalog_choice() {
        let choices = vec![
            ModelChoice {
                label: "Shared".to_string(),
                key: "shared-model".to_string(),
                provider_id: "provider-a".to_string(),
                family: "A".to_string(),
            },
            ModelChoice {
                label: "Shared".to_string(),
                key: "shared-model".to_string(),
                provider_id: "provider-b".to_string(),
                family: "B".to_string(),
            },
        ];
        assert!(resolve_unique_model_choice(&choices, "shared").is_none());
        assert!(resolve_unique_model_choice(&choices, "missing").is_none());

        let unique = ModelChoice {
            label: "Catalog Model".to_string(),
            key: "catalog-model".to_string(),
            provider_id: "catalog-provider".to_string(),
            family: "Catalog".to_string(),
        };
        let selected = resolve_unique_model_choice(std::slice::from_ref(&unique), "catalog-model").unwrap();
        let mut cfg = make_openai_config("previous-model");
        apply_model_choice(&mut cfg, &selected);
        assert_eq!(cfg.config.active_alias, "catalog-model");
        assert_eq!(cfg.config.active_provider_id, "catalog-provider");
    }
}
