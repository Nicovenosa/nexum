    async fn render_headless_model_no_provider() -> (App, crate::ui::headless::HeadlessHandle) {
        let (mut app, mut handle) = App::new_headless(120, 30).await;
        let cfg = crate::config::NexumConfig {
            schema: None,
            config: crate::config::AppConfig {
                active_alias: "opus".to_string(),
                active_provider_id: "test".to_string(),
                providers: vec![crate::config::ProviderConfig {
                    id: "test".to_string(),
                    ..Default::default()
                }],
                thinking: Some(crate::config::ThinkingConfig {
                    enabled: true,
                    budget_tokens: 8000,
                    effort: "medium".to_string(),
                    max_tokens: 32000,
                }),
                ..Default::default()
            },
        };
        let panel = ModelPanel::from_config(&cfg);
        app.session_mgr.current_mut()
            .session_panels
            .open(crate::app::panel_manager::PanelState::Model(panel.clone()));
        handle
            .terminal
            .draw(|f| crate::ui::main_ui::render(f, &mut app))
            .unwrap();
        (app, handle)
    }

    async fn render_headless_openai_model() -> (App, crate::ui::headless::HeadlessHandle) {
        let (mut app, mut handle) = App::new_headless(120, 30).await;
        let cfg = crate::config::NexumConfig {
            schema: None,
            config: crate::config::AppConfig {
                active_alias: "deepseek-v4-flash".to_string(),
                active_provider_id: "openai-compatible".to_string(),
                providers: vec![crate::config::ProviderConfig {
                    id: "openai-compatible".to_string(),
                    provider_type: "openai".to_string(),
                    name: Some("OpenAI".to_string()),
                    ..Default::default()
                }],
                thinking: Some(crate::config::ThinkingConfig {
                    enabled: true,
                    budget_tokens: 8000,
                    effort: "medium".to_string(),
                    max_tokens: 32000,
                }),
                ..Default::default()
            },
        };
        let panel = ModelPanel::from_config_with_openai_env(&cfg, None, Some("deepseek-v4-flash"));
        *app.services.nexum_config.write() = cfg;
        app.session_mgr.current_mut()
            .session_panels
            .open(crate::app::panel_manager::PanelState::Model(panel.clone()));
        handle
            .terminal
            .draw(|f| crate::ui::main_ui::render(f, &mut app))
            .unwrap();
        (app, handle)
    }

    async fn render_headless_ollama_model() -> (App, crate::ui::headless::HeadlessHandle) {
        // Altura 220: el catálogo real crece con cada provider conectado
        // (post autologin 2026-07-06: + Claude 14 modelos + Codex 7). Con 80
        // filas las filas de config de Ollama quedaban fuera del viewport y
        // el test fallaba según el estado de la máquina. 220 da margen para
        // ~50 modelos + headers + config.
        // Aislado: sin esto el test lee el catálogo de quien lo corra, y la
        // aserción "el modelo tiene que venir del catálogo" pasa o falla según
        // qué providers tenga conectados esa máquina. Es la misma deuda que ya
        // estaba anotada en los tests de model_panel.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
        let (mut app, mut handle) = App::new_headless(120, 220).await;
        let cfg = crate::config::NexumConfig {
            schema: None,
            config: crate::config::AppConfig {
                active_alias: "qwen2.5:0.5b".to_string(),
                active_provider_id: "ollama-local".to_string(),
                providers: vec![crate::config::ProviderConfig {
                    id: "ollama-local".to_string(),
                    provider_type: "openai".to_string(),
                    base_url: "http://127.0.0.1:11434/v1".to_string(),
                    name: Some("Ollama Local".to_string()),
                    ..Default::default()
                }],
                thinking: Some(crate::config::ThinkingConfig {
                    enabled: true,
                    budget_tokens: 8000,
                    effort: "medium".to_string(),
                    max_tokens: 32000,
                }),
                context_1m: Some(true),
                ..Default::default()
            },
        };
        let panel = ModelPanel::from_config_with_openai_env(
            &cfg,
            Some("qwen2.5:0.5b"),
            Some("qwen2.5:0.5b"),
        );
        *app.services.nexum_config.write() = cfg;
        app.session_mgr
            .current_mut()
            .session_panels
            .open(crate::app::panel_manager::PanelState::Model(panel));
        handle
            .terminal
            .draw(|f| crate::ui::main_ui::render(f, &mut app))
            .unwrap();
        (app, handle)
    }

    #[tokio::test]
    async fn test_model_panel_renders_select_model_title() {
        let (_, handle) = render_headless_model_no_provider().await;
        let snap = handle.snapshot().join("\n");
        assert!(
            snap.contains("Select model"),
            "Panel should show 'Select model' title, got:\n{}",
            snap
        );
    }

    #[tokio::test]
    async fn test_model_panel_shows_effort() {
        let (_, handle) = render_headless_model_no_provider().await;
        let snap = handle.snapshot().join("\n");
        assert!(
            snap.contains("Effort"),
            "Panel should show effort setting, got:\n{}",
            snap
        );
    }

    /// Recorta el snapshot a las líneas del PANEL de modelos.
    ///
    /// Estos tests assertaban sobre la pantalla entera, y detrás del panel se
    /// dibujan el welcome card y la status bar, que muestran el modelo activo
    /// del runtime **por diseño**. Con eso, el test fallaba por
    /// `OpenAI · deepseek-v4-flash` en la barra de estado — que no es un modelo
    /// que el panel se haya inventado, es el que está en uso.
    ///
    /// El resultado dependía de qué default tuviera el entorno en ese momento,
    /// así que el test era flaky y, cuando pasaba, no probaba lo que decía.
    fn solo_el_panel(handle: &crate::ui::headless::HeadlessHandle) -> String {
        let lineas = handle.snapshot();
        let inicio = lineas
            .iter()
            .position(|l| l.contains("Switch between models") || l.contains("modelos"))
            .unwrap_or(0);
        lineas[inicio..]
            .iter()
            .take_while(|l| !l.trim_start().starts_with("────"))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn test_openai_model_panel_does_not_render_fabricated_models() {
        // Aislado: sin esto el test lee el catálogo de quien lo corra.
        // El candado va ANTES del guard: XDG_DATA_HOME es estado global.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
        let (_, handle) = render_headless_openai_model().await;
        let snap = solo_el_panel(&handle);
        assert!(
            !snap.contains("deepseek-v4-flash"),
            "Panel must not show a model absent from the catalog, got:\n{}",
            snap
        );
        assert!(
            !snap.contains("Opus") && !snap.contains("Sonnet") && !snap.contains("Haiku"),
            "Panel should not show Claude aliases for OpenAI-compatible providers, got:\n{}",
            snap
        );
    }

    #[tokio::test]
    async fn test_ollama_model_panel_shows_local_metadata_without_model_fallback() {
        let (_, handle) = render_headless_ollama_model().await;
        let snap = handle.snapshot().join("\n");
        assert!(
            !snap.contains("qwen2.5:0.5b"),
            "Ollama model must come from the catalog, got:\n{}",
            snap
        );
        assert!(
            snap.contains("Max Token: local")
                && snap.contains("Effort: local")
                && snap.contains("1M Context: local"),
            "Ollama model panel should mark local metadata instead of fake presets, got:\n{}",
            snap
        );
        assert!(
            !snap.contains("1M ContextON") && !snap.contains("1M ContextOFF"),
            "Ollama model panel should not show fake 1M ON/OFF metadata, got:\n{}",
            snap
        );
    }

    #[tokio::test]
    async fn test_model_panel_has_solid_popup_background() {
        let (_, handle) = render_headless_ollama_model().await;
        let buffer = handle.terminal.backend().buffer();
        let solid_cells = buffer
            .content
            .iter()
            .filter(|cell| cell.symbol() != " " && cell.bg == crate::ui::theme::POPUP_BG)
            .count();
        assert!(
            solid_cells > 20,
            "Model panel text/border cells should have solid popup background"
        );
    }
