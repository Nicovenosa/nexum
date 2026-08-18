use ratatui::crossterm::event::KeyCode;

use super::super::Action;
use super::{
    SHORTCUT_BG_BAR, SHORTCUT_CTRL_CYCLE_MODE, SHORTCUT_CTRL_CYCLE_PROVIDER, SHORTCUT_CYCLE_MODE,
    SHORTCUT_CYCLE_PROVIDER,
};
use crate::app::{model_panel::ModelPanel, App, MessageViewModel};

/// 处理全局快捷键：Ctrl+B（bg bar）、Ctrl+P（aprobación visual）、Ctrl+T（模型切换）、Ctrl+Shift+T（Provider 切换）、Ctrl+O（diff 切换）
pub(super) fn handle_shortcuts(
    app: &mut App,
    key_event: &ratatui::crossterm::event::KeyEvent,
) -> Option<Action> {
    // Ctrl+P: cycle visual approval mode. This does NOT change permission_mode
    // and does NOT enable real auto-approval.
    if key_event
        .modifiers
        .contains(ratatui::crossterm::event::KeyModifiers::CONTROL)
        && matches!(key_event.code, KeyCode::Char('p'))
    {
        app.global_ui.approval_display_mode = app.global_ui.approval_display_mode.next();
        app.global_ui.approval_highlight_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(1500));
        return Some(Action::Redraw);
    }

    // Ctrl+S / F6: alternar modo selección de texto (copy parcial, sprint
    // 2026-07-07). En ON, run_app desactiva el mouse capture del terminal
    // para que la selección nativa (arrastrar con el mouse) funcione; en OFF,
    // re-activa el capture y el scroll interno de la TUI. La reconciliación
    // del capture la hace run_app comparando global_ui.selection_mode con el
    // estado ya aplicado.
    if (key_event
        .modifiers
        .contains(ratatui::crossterm::event::KeyModifiers::CONTROL)
        && matches!(key_event.code, KeyCode::Char('s')))
        || matches!(key_event.code, KeyCode::F(6))
    {
        app.global_ui.selection_mode = !app.global_ui.selection_mode;
        return Some(Action::Redraw);
    }

    // Ctrl+O: toggle inline diff (only when OAuth popup is NOT active)
    if key_event
        .modifiers
        .contains(ratatui::crossterm::event::KeyModifiers::CONTROL)
        && matches!(key_event.code, KeyCode::Char('o'))
    {
        if app.global_ui.oauth_prompt.is_none() {
            app.toggle_diff();
        }
        return Some(Action::Redraw);
    }

    // Ctrl+B: 跳转到后台 agent bar
    if SHORTCUT_BG_BAR.matches(key_event) {
        if !app.session_mgr.current_mut().background_agents.is_empty() {
            app.session_mgr.current_mut().ui.bg_bar_cursor = Some(0);
        }
        return Some(Action::Redraw);
    }

    // Ctrl+T / Alt+M: cycle model aliases
    if SHORTCUT_CTRL_CYCLE_MODE.matches(key_event) || SHORTCUT_CYCLE_MODE.matches(key_event) {
        {
            let cfg_arc = app.services.nexum_config.clone();
            let mut cfg = cfg_arc.write();
            let panel = ModelPanel::from_config(&cfg);
            let current = cfg.config.active_alias.as_str();
            let choices: Vec<String> = (0..panel.model_choice_count())
                .filter_map(|row| panel.model_choice(row).map(|choice| choice.key.clone()))
                .collect();
            let next = choices
                .iter()
                .position(|choice| choice == current)
                .map(|idx| choices[(idx + 1) % choices.len()].clone())
                .or_else(|| choices.first().cloned())
                .unwrap_or_else(|| current.to_string());
            cfg.config.active_alias = next.clone();
            if let Err(e) = App::save_config(&cfg, app.services.config_path_override.as_deref()) {
                app.session_mgr.current_mut().messages.view_messages.push(
                    MessageViewModel::system(app.services.lc.tr_args(
                        "config-save-failed",
                        &[("error".into(), e.to_string().into())],
                    )),
                );
            }
            if let Some(p) = crate::app::agent::LlmProvider::from_config(&cfg) {
                app.services.provider_name =
                    crate::app::model_panel::provider_display_name(&cfg)
                        .unwrap_or_else(|| p.display_name().to_string());
                app.services.model_name = p.model_name().to_string();
            }
            if let Some(ref acp_client) = app.acp_client {
                let acp = acp_client.clone();
                let alias = next.clone();
                tokio::spawn(async move {
                    let _ = acp.set_config_option("model", &alias).await;
                });
            }
            app.global_ui.model_highlight_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(1500));
        }
        return Some(Action::Redraw);
    }

    // Ctrl+Shift+T / Alt+Shift+M: cycle providers
    if SHORTCUT_CTRL_CYCLE_PROVIDER.matches(key_event) || SHORTCUT_CYCLE_PROVIDER.matches(key_event)
    {
        {
            let cfg_arc = app.services.nexum_config.clone();
            let mut cfg = cfg_arc.write();
            let providers_len = cfg.config.providers.len();
            if providers_len > 1 {
                let current_id = cfg.config.active_provider_id.as_str();
                let next_id = {
                    let providers = &cfg.config.providers;
                    let idx = providers
                        .iter()
                        .position(|p| p.id == current_id)
                        .unwrap_or(0);
                    let next_idx = (idx + 1) % providers.len();
                    providers[next_idx].id.clone()
                };
                cfg.config.active_provider_id = next_id;
                if let Some(p) = crate::app::agent::LlmProvider::from_config(&cfg) {
                    app.services.provider_name =
                        crate::app::model_panel::provider_display_name(&cfg)
                            .unwrap_or_else(|| p.display_name().to_string());
                    app.services.model_name = p.model_name().to_string();
                }
                if let Err(e) = App::save_config(&cfg, app.services.config_path_override.as_deref())
                {
                    app.session_mgr.current_mut().messages.view_messages.push(
                        MessageViewModel::system(app.services.lc.tr_args(
                            "config-save-failed",
                            &[("error".into(), e.to_string().into())],
                        )),
                    );
                }
                app.global_ui.provider_highlight_until =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(2000));
            }
        }
        return Some(Action::Redraw);
    }

    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::RwLock;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::tempdir;

    use super::*;
    use crate::config::{AppConfig, NexumConfig, ProviderConfig, ThinkingConfig};

    fn make_openai_config() -> NexumConfig {
        NexumConfig {
            schema: None,
            config: AppConfig {
                active_alias: "deepseek-v4-flash".to_string(),
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

    #[tokio::test]
    async fn test_ctrl_t_keeps_openai_real_model_instead_of_claude_alias() {
        let (mut app, _) = App::new_headless(80, 24).await;
        app.services.nexum_config = Arc::new(RwLock::new(make_openai_config()));
        app.services.config_path_override = Some(tempdir().unwrap().keep().join("settings.json"));

        let key = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
        let action = handle_shortcuts(&mut app, &key);

        assert!(action.is_some(), "Ctrl+T should be handled");
        let alias = app.services.nexum_config.read().config.active_alias.clone();
        // Multi-provider: Ctrl+T cicla entre todos los modelos disponibles.
        // Verificamos que el alias no es un alias de Claude legacy.
        assert!(
            !matches!(alias.as_str(), "opus" | "sonnet" | "haiku"),
            "Ctrl+T should not cycle to Claude legacy aliases, got: {alias}"
        );
    }

    #[tokio::test]
    async fn test_tab_cycles_visual_agent_mode() {
        let (mut app, _) = App::new_headless(80, 24).await;
        assert_eq!(app.global_ui.agent_mode, crate::app::AgentMode::Build);

        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let action = crate::event::keyboard::handle_key_event(&mut app, key)
            .expect("Tab debe procesarse sin error");

        assert!(action.is_some(), "Tab should be handled");
        assert_eq!(app.global_ui.agent_mode, crate::app::AgentMode::Plan);
    }

    #[tokio::test]
    async fn test_backtab_cycles_visual_agent_mode_backward_without_permission_change() {
        let (mut app, _) = App::new_headless(80, 24).await;
        let before = app.services.permission_mode.load();
        assert_eq!(app.global_ui.agent_mode, crate::app::AgentMode::Build);

        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        let action = crate::event::keyboard::handle_key_event(&mut app, key)
            .expect("BackTab debe procesarse sin error");

        assert!(action.is_some(), "BackTab should be handled");
        assert_eq!(app.global_ui.agent_mode, crate::app::AgentMode::Research);
        assert_eq!(app.services.permission_mode.load(), before);
    }

    #[tokio::test]
    async fn test_ctrl_p_cycles_visual_approval_without_permission_change() {
        let (mut app, _) = App::new_headless(80, 24).await;
        let before = app.services.permission_mode.load();
        assert_eq!(
            app.global_ui.approval_display_mode,
            crate::app::ApprovalDisplayMode::Manual
        );

        let key = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        let action = handle_shortcuts(&mut app, &key);

        assert!(action.is_some(), "Ctrl+P should be handled");
        assert_eq!(
            app.global_ui.approval_display_mode,
            crate::app::ApprovalDisplayMode::Partial
        );
        assert_eq!(app.services.permission_mode.load(), before);
    }

    #[tokio::test]
    async fn test_ctrl_s_toggles_selection_mode() {
        let mut app = App::new_headless(80, 24).await.0;
        assert!(!app.global_ui.selection_mode, "default off");

        let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        let action = handle_shortcuts(&mut app, &key);
        assert!(action.is_some(), "Ctrl+S debe ser manejado");
        assert!(app.global_ui.selection_mode, "Ctrl+S activa el modo selección");

        // Segundo Ctrl+S vuelve a off.
        handle_shortcuts(&mut app, &key);
        assert!(!app.global_ui.selection_mode, "Ctrl+S de nuevo desactiva");
    }

    #[tokio::test]
    async fn test_f6_tambien_toggles_selection_mode() {
        let mut app = App::new_headless(80, 24).await.0;
        let key = KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE);
        handle_shortcuts(&mut app, &key);
        assert!(app.global_ui.selection_mode, "F6 activa el modo selección");
    }
}
