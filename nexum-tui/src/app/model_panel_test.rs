use super::*;
use crate::config::{AppConfig, ProviderConfig};

fn make_config() -> NexumConfig {
    NexumConfig {
        schema: None,
        config: AppConfig {
            active_alias: "opus".to_string(),
            active_provider_id: "test".to_string(),
            providers: vec![ProviderConfig {
                id: "test".to_string(),
                name: Some("TestProvider".to_string()),
                ..Default::default()
            }],
            thinking: Some(ThinkingConfig {
                enabled: false,
                budget_tokens: 8000,
                effort: "medium".to_string(),
                max_tokens: 32000,
            }),
            ..Default::default()
        },
    }
}

fn make_openai_config() -> NexumConfig {
    NexumConfig {
        schema: None,
        config: AppConfig {
            active_alias: "opus".to_string(),
            active_provider_id: "openai-compatible".to_string(),
            providers: vec![ProviderConfig {
                id: "openai-compatible".to_string(),
                provider_type: "openai".to_string(),
                api_key: "test-key".to_string(),
                name: Some("OpenAI".to_string()),
                ..Default::default()
            }],
            thinking: Some(ThinkingConfig {
                enabled: false,
                budget_tokens: 8000,
                effort: "medium".to_string(),
                max_tokens: 32000,
            }),
            ..Default::default()
        },
    }
}

#[test]
fn test_openai_model_panel_does_not_invent_model_without_catalog() {
        // Aislado: sin esto el test lee el catálogo de quien lo corra.
        // El candado va ANTES del guard: XDG_DATA_HOME es estado global.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
    let cfg = make_openai_config();
    let panel = ModelPanel::from_config_with_openai_env(&cfg, None, Some("deepseek-v4-flash"));
    let labels = panel.model_choice_labels();
    assert!(labels.is_empty());
    assert_eq!(panel.active_model_label(), "");
}

#[test]
fn test_openai_model_panel_ignores_models_env_without_catalog() {
        // Aislado: sin esto el test lee el catálogo de quien lo corra.
        // El candado va ANTES del guard: XDG_DATA_HOME es estado global.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
    let cfg = make_openai_config();
    let panel = ModelPanel::from_config_with_openai_env(
        &cfg,
        Some("deepseek-v4-flash, qwen-max"),
        Some("ignored-model"),
    );
    let labels = panel.model_choice_labels();
    assert!(labels.is_empty());
}

#[test]
fn test_openai_model_panel_has_no_default_without_catalog() {
        // Aislado: sin esto el test lee el catálogo de quien lo corra.
        // El candado va ANTES del guard: XDG_DATA_HOME es estado global.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
    let cfg = make_openai_config();
    let panel = ModelPanel::from_config_with_openai_env(&cfg, None, None);
    let labels = panel.model_choice_labels();
    assert!(labels.is_empty());
}

#[test]
fn test_from_config_defaults() {
        // Aislado: sin esto el test lee el catálogo de quien lo corra.
        // El candado va ANTES del guard: XDG_DATA_HOME es estado global.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
    let cfg = make_config();
    let panel = ModelPanel::from_config(&cfg);
    assert_eq!(panel.active_tab, AliasTab::Opus);
    assert_eq!(panel.model_choice_count(), 0);
    assert!(!panel.is_model_row(panel.cursor()));
    assert_eq!(panel.provider_name, "TestProvider");
    assert_eq!(panel.buf_thinking_effort, "medium");
}

#[test]
fn test_from_config_sonnet() {
        // Aislado: sin esto el test lee el catálogo de quien lo corra.
        // El candado va ANTES del guard: XDG_DATA_HOME es estado global.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
    let mut cfg = make_config();
    cfg.config.active_alias = "sonnet".to_string();
    let panel = ModelPanel::from_config(&cfg);
    assert_eq!(panel.active_tab, AliasTab::Sonnet);
    assert_eq!(panel.model_choice_count(), 0);
}

#[test]
fn test_move_cursor_clamp() {
        // Aislado: sin esto el test lee el catálogo de quien lo corra.
        // El candado va ANTES del guard: XDG_DATA_HOME es estado global.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
    let cfg = make_config();
    let mut panel = ModelPanel::from_config(&cfg);
    let initial_cursor = panel.cursor();
    assert!(!panel.is_model_row(initial_cursor));
    panel.move_cursor(1, 1);
    assert_eq!(panel.cursor(), panel.effort_row());
    panel.move_cursor(-1, 1);
    assert_eq!(panel.cursor(), panel.effort_row());
}

#[test]
fn test_cycle_effort() {
    let cfg = make_config();
    let mut panel = ModelPanel::from_config(&cfg);

    assert_eq!(panel.buf_thinking_effort, "medium");
    panel.cycle_effort(false);
    assert_eq!(panel.buf_thinking_effort, "high");
    panel.cycle_effort(false);
    assert_eq!(panel.buf_thinking_effort, "xhigh");
    panel.cycle_effort(false);
    assert_eq!(panel.buf_thinking_effort, "max");
    panel.cycle_effort(false);
    assert_eq!(panel.buf_thinking_effort, "low");
    panel.cycle_effort(false);
    assert_eq!(panel.buf_thinking_effort, "medium");

    panel.cycle_effort(true);
    assert_eq!(panel.buf_thinking_effort, "low");
    panel.cycle_effort(true);
    assert_eq!(panel.buf_thinking_effort, "max");
    panel.cycle_effort(true);
    assert_eq!(panel.buf_thinking_effort, "xhigh");
    panel.cycle_effort(true);
    assert_eq!(panel.buf_thinking_effort, "high");
}

#[test]
fn test_cycle_effort_works_from_any_row() {
    let cfg = make_config();
    let mut panel = ModelPanel::from_config(&cfg);
    panel.cycle_effort(false);
    assert_eq!(panel.buf_thinking_effort, "high");
}

#[test]
fn test_apply_to_config() {
    let cfg = make_config();
    let mut panel = ModelPanel::from_config(&cfg);
    panel.active_tab = AliasTab::Sonnet;
    panel.active_model_key = "sonnet".to_string();
    panel.buf_thinking_effort = "high".to_string();
    panel.active_provider_id = "new-provider".to_string();

    let mut cfg2 = make_config();
    panel.apply_to_config(&mut cfg2);
    assert_eq!(cfg2.config.active_alias, "sonnet");
    assert_eq!(cfg2.config.active_provider_id, "new-provider");
    assert!(cfg2.config.thinking.as_ref().unwrap().enabled);
    assert_eq!(cfg2.config.thinking.as_ref().unwrap().effort, "high");
}

#[test]
fn test_selected_catalog_model_persists_exact_key_and_provider() {
    let choices = vec![
        ModelChoice {
            label: "Opus".to_string(),
            key: "opus".to_string(),
            provider_id: "legacy-provider".to_string(),
            family: "Legacy".to_string(),
        },
        ModelChoice {
            label: "Catalog Arbitrary".to_string(),
            key: "catalog-model-42".to_string(),
            provider_id: "catalog-provider".to_string(),
            family: "Catalog".to_string(),
        },
    ];
    let display_rows = build_display_rows_from_choices(&choices);
    let mut panel = ModelPanel {
        provider_name: "Catalog".to_string(),
        catalog_error: None,
        active_tab: AliasTab::Sonnet,
        model_choices: choices,
        display_rows,
        active_model_key: "opus".to_string(),
        active_provider_id: "legacy-provider".to_string(),
        buf_thinking_effort: "high".to_string(),
        buf_max_tokens: 32000,
        buf_context_1m: false,
        cursor: 2,
        scroll_offset: 0,
    };
    panel.select_model_row(2);

    let mut cfg = make_config();
    panel.apply_to_config(&mut cfg);

    assert_eq!(cfg.config.active_alias, "catalog-model-42");
    assert_eq!(cfg.config.active_provider_id, "catalog-provider");
    assert_eq!(panel.active_tab, AliasTab::Sonnet);
}

#[test]
fn test_apply_to_config_creates_thinking_when_none() {
    let mut cfg = NexumConfig {
        schema: None,
        config: AppConfig {
            active_alias: "opus".to_string(),
            active_provider_id: "test".to_string(),
            providers: vec![ProviderConfig {
                id: "test".to_string(),
                ..Default::default()
            }],
            thinking: None,
            ..Default::default()
        },
    };
    let panel = ModelPanel::from_config(&cfg);
    panel.apply_to_config(&mut cfg);
    let t = cfg.config.thinking.as_ref().unwrap();
    assert!(t.enabled);
    assert_eq!(t.effort, "high");
}

#[test]
fn test_reserved_internal_model_does_not_create_fallback_choices() {
        // Aislado: sin esto el test lee el catálogo de quien lo corra.
        // El candado va ANTES del guard: XDG_DATA_HOME es estado global.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
    // qwen3:0.6b is reserved for the Hormiguero and must NOT appear as a
    // user-facing choice, even when OPENAI_MODELS lists it.
    let cfg = make_openai_config();
    let panel = ModelPanel::from_config_with_openai_env(
        &cfg,
        Some("qwen2.5:0.5b,qwen3:0.6b,qwen2.5:1.5b"),
        None,
    );
    let labels = panel.model_choice_labels();
    assert!(labels.is_empty());
}

#[test]
fn test_reserved_model_does_not_create_default_fallback() {
        // Aislado: sin esto el test lee el catálogo de quien lo corra.
        // El candado va ANTES del guard: XDG_DATA_HOME es estado global.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
    // If the only configured model were reserved, /modelo must not expose it.
    // With multi-provider catalog, the catalog provides models instead of fallback.
    let cfg = make_openai_config();
    let panel =
        ModelPanel::from_config_with_openai_env(&cfg, Some("qwen3:0.6b"), Some("qwen3:0.6b"));
    let labels = panel.model_choice_labels();
    assert!(
        !labels.contains(&"qwen3:0.6b".to_string()),
        "reserved model exposed as choice: {labels:?}"
    );
    assert!(labels.is_empty());
}

#[test]
fn test_catalog_models_require_usable_provider_even_for_opencode() {
    let catalog = serde_json::json!({
        "providers": [
            {
                "id": "opencode_zen",
                "family": "OpenCode Zen",
                "usable_now": false,
                "models": ["hidden-open-code-model"]
            },
            {
                "id": "opencode_go",
                "family": "OpenCode Go",
                "usable_now": true,
                "models": ["selectable-open-code-model"]
            }
        ]
    });
    let (choices, _) = build_model_choices_from_catalog_value(&catalog).unwrap();
    let labels: Vec<&str> = choices.iter().map(|choice| choice.label.as_str()).collect();
    assert!(!labels.contains(&"hidden-open-code-model"));
    assert!(labels.contains(&"selectable-open-code-model"));
}

#[test]
fn test_catalog_only_model_choices_require_usable_catalog_models() {
    let absent = catalog_model_choices(None);
    assert!(absent.0.is_empty(), "sin catálogo no hay modelos seleccionables");

    let nonconfigured = serde_json::json!({
        "providers": [{"id": "p", "status": "not_configured", "models": ["inventado"]}]
    });
    assert!(catalog_model_choices(Some(&nonconfigured)).0.is_empty());

    let login_not_usable = serde_json::json!({
        "providers": [{"id": "p", "native_login_detected": true, "usable_now": false, "models": ["inventado"]}]
    });
    assert!(catalog_model_choices(Some(&login_not_usable)).0.is_empty());

    let usable_without_models = serde_json::json!({
        "providers": [{"id": "p", "usable_now": true, "models": []}]
    });
    assert!(catalog_model_choices(Some(&usable_without_models)).0.is_empty());

    let usable = serde_json::json!({
        "providers": [{"id": "p", "family": "Proveedor", "usable_now": true, "models": ["catalog-model"]}]
    });
    let choices = catalog_model_choices(Some(&usable)).0;
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].key, "catalog-model");
}

#[test]
fn test_model_panel_contains_no_checkout_credential_resolver() {
    let source = include_str!("model_panel.rs");
    assert!(!source.contains("resolve_cli_root"));
    assert!(!source.contains("current_dir"));
    // El script `provider_resolve.py` se sigue invocando — lo que cambió es
    // QUIÉN resuelve su ruta. Assertar sobre su nombre dejó de distinguir
    // checkout de instalado (aparece en comentarios), así que se assertá sobre
    // el resolvedor, que es la propiedad real y es una condición más fuerte.
    assert!(source.contains("installed_provider_resolver_path"));
}

#[test]
fn test_alias_tab_description() {
    assert_eq!(
        AliasTab::Opus.description(),
        "Most capable for complex work"
    );
    assert_eq!(
        AliasTab::Sonnet.description(),
        "Balanced performance and speed"
    );
    assert_eq!(AliasTab::Haiku.description(), "Fastest for quick answers");
}

// ─── Fix UX popups (2026-07-05) — Bugs 1/2/3 ─────────────────────────────────

/// Bug 1: 40 ítems, 30 bajadas de cursor → el ítem 30 queda seleccionado y
/// dentro del viewport (el offset lo sigue).
#[test]
fn test_scroll_40_items_cursor_30_dentro_del_viewport() {
    let choices: Vec<ModelChoice> = (0..40)
        .map(|i| ModelChoice {
            label: format!("modelo-{i}"),
            key: format!("modelo-{i}"),
            provider_id: "test-prov".to_string(),
            family: "Test".to_string(),
        })
        .collect();
    let display_rows = build_display_rows_from_choices(&choices);
    let mut panel = ModelPanel {
        provider_name: "Test".to_string(),
        catalog_error: None,
        active_tab: AliasTab::Opus,
        model_choices: choices,
        display_rows,
        active_model_key: "modelo-0".to_string(),
        active_provider_id: "test-prov".to_string(),
        buf_thinking_effort: "high".to_string(),
        buf_max_tokens: 32000,
        buf_context_1m: false,
        cursor: 1, // primer fila de modelo (0 es el header)
        scroll_offset: 0,
    };

    for _ in 0..30 {
        panel.move_cursor(1, 1);
    }
    // Header en fila 0 → el ítem 30 está en la fila 31.
    assert_eq!(panel.cursor(), 31, "el ítem 30 debe quedar seleccionado");

    // Simular el seguimiento del viewport como hace el render (2 líneas de
    // encabezado del popup + header de sección antes de la lista).
    let viewport = 12usize;
    let total = 2 + panel.display_row_count() + 5;
    let cursor_line = 2 + panel.cursor();
    let offset = crate::ui::main_ui::panels::scroll::ensure_visible(
        panel.scroll_offset,
        cursor_line,
        cursor_line,
        viewport,
        total,
    );
    panel.scroll_offset = offset;
    assert!(
        cursor_line >= offset && cursor_line < offset + viewport,
        "ítem 30 fuera del viewport: line={cursor_line} offset={offset}"
    );
}

/// Bug 1: PgDn/G mueven el cursor de a saltos y hasta el final.
#[test]
fn test_page_y_extremos_mueven_cursor() {
    let cfg = make_openai_config();
    let mut panel = ModelPanel::from_config_with_openai_env(&cfg, None, Some("m-activo"));
    panel.cursor_to_first();
    let first = panel.cursor();
    panel.move_cursor(1, 8);
    assert!(panel.cursor() > first, "PgDn debe avanzar el cursor");
    panel.cursor_to_last();
    assert_eq!(
        panel.cursor(),
        panel.context_1m_row(),
        "G/End debe ir al último ítem (fila 1M context)"
    );
    panel.cursor_to_first();
    assert_eq!(panel.cursor(), first, "g/Home debe volver al primero");
}

/// Bug 2 (unitario): resolver credenciales → upsert → LlmProvider::from_config
/// devuelve el provider correcto (ID histórico opencode_zen), no None/Ollama.
#[test]
fn test_upsert_resolved_provider_habilita_from_config() {
    let mut cfg = make_openai_config();
    cfg.config.active_provider_id = "opencode_zen".to_string();
    cfg.config.active_alias = "deepseek-v4-flash-free".to_string();
    assert!(
        crate::app::agent::LlmProvider::from_config(&cfg).is_none(),
        "precondición: sin ProviderConfig para opencode_zen, from_config es None"
    );

    upsert_resolved_provider(
        &mut cfg,
        &ResolvedProvider {
            provider_id: "opencode_zen".to_string(),
            display_name: "OpenCode Zen".to_string(),
            base_url: "https://opencode.ai/zen/v1".to_string(),
            api_key: "sk-zen-test-123".to_string(),
            protocol: "openai".to_string(),
        },
    );

    // Integración: la función que decide a qué provider va el prompt.
    match crate::app::agent::LlmProvider::from_config(&cfg) {
        Some(crate::app::agent::LlmProvider::OpenAi {
            base_url, model, ..
        }) => {
            assert_eq!(base_url, "https://opencode.ai/zen/v1");
            assert_eq!(model, "deepseek-v4-flash-free");
        }
        other => panic!("se esperaba OpenAi de Zen, hubo {:?}", other.is_some()),
    }
}

#[test]
fn installed_release_never_resolves_provider_from_src() {
    let tmp = tempfile::tempdir().unwrap();
    let slot = tmp.path().join("slot");
    std::fs::create_dir_all(slot.join("src/nexum_providers")).unwrap();
    std::fs::write(slot.join("src/nexum_providers/provider_resolve.py"), "").unwrap();
    let executable = slot.join("nexum");
    std::fs::write(&executable, "").unwrap();
    let error = nexum_acp::provider::routes::provider_resolver_for_executable(&executable)
        .unwrap_err()
        .to_string();
    assert!(error.contains("libexec/nexum/providers"));
    assert!(!error.contains("src/nexum_providers"));
}

#[test]
fn opencode_resolver_is_present_in_installed_slot() {
    let tmp = tempfile::tempdir().unwrap();
    let slot = tmp.path().join("slot");
    let resolver =
        slot.join(nexum_acp::provider::routes::INSTALLED_PROVIDER_RESOLVER);
    std::fs::create_dir_all(resolver.parent().unwrap()).unwrap();
    std::fs::write(&resolver, "# fixture").unwrap();
    let executable = slot.join("nexum");
    std::fs::write(&executable, "").unwrap();
    assert_eq!(
        nexum_acp::provider::routes::provider_resolver_for_executable(&executable).unwrap(),
        resolver
    );
}

#[test]
fn opencode_free_does_not_require_http_api_key() {
    let registry = nexum_acp::provider::routes::ProviderRouteRegistry::load_from_path(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../config/provider-route-registry.json"),
    )
    .unwrap();
    assert_eq!(
        registry.route("opencode_zen").unwrap().auth_mode,
        "cli_account"
    );
}

#[test]
fn opencode_go_does_not_require_wrong_auth_mode() {
    let registry = nexum_acp::provider::routes::ProviderRouteRegistry::load_from_path(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../config/provider-route-registry.json"),
    )
    .unwrap();
    assert_eq!(
        registry.route("opencode_go").unwrap().auth_mode,
        "cli_account"
    );
}

#[test]
fn opencode_activation_error_is_not_rewritten_as_api_key_error() {
    let error = provider_activation_error(
        "opencode_zen",
        "deepseek-v4-flash-free",
        "PROVIDER_ROUTE_REGISTRY_NOT_FOUND",
    );
    assert!(error.contains("PROVIDER_ACTIVATION_FAILED"));
    assert!(error.contains("provider_id=opencode_zen"));
    assert!(error.contains("model_id=deepseek-v4-flash-free"));
    assert!(!error.contains("No hay API Key"));
    assert!(!error.contains("/login"));
}

/// Bug 2 (UI): la statusbar lee provider_name — debe reflejar el nombre
/// validado del provider activo ("OpenCode Free"), no el genérico.
#[test]
fn test_provider_display_name_para_statusbar() {
    // El catálogo va explícito. Antes este test leía el que encontrara —el de
    // la máquina o el del checkout— y pasaba por dónde estaba parado el repo,
    // no por lo que afirma.
    let _env = crate::ui::demo_mode::test_env_lock();
    let _catalogo = crate::app::catalog_fixture::CatalogoAislado::con(serde_json::json!({
        "schema_version": 2,
        "providers": [{
            "id": "opencode_zen",
            "display_name": "OpenCode Free",
            "family": "OpenCode Free",
            "usable_now": true,
            "models": ["deepseek-v4-flash-free"]
        }]
    }));
    let mut cfg = make_openai_config();
    cfg.config.active_provider_id = "opencode_zen".to_string();
    cfg.config.active_alias = "deepseek-v4-flash-free".to_string();
    upsert_resolved_provider(
        &mut cfg,
        &ResolvedProvider {
            provider_id: "opencode_zen".to_string(),
            display_name: "OpenCode Zen".to_string(),
            base_url: "https://opencode.ai/zen/v1".to_string(),
            api_key: "sk-zen-test-123".to_string(),
            protocol: "openai".to_string(),
        },
    );
    assert_eq!(provider_display_name(&cfg).as_deref(), Some("OpenCode Free"));
}

#[test]
fn footer_model_exists_in_loaded_catalog() {
    let mut cfg = make_openai_config();
    cfg.config.active_provider_id = "codex_cli".to_string();
    cfg.config.active_alias = "gpt-5.6-terra".to_string();
    let (provider, model) = crate::app::runtime_identity::statusbar_identity(&cfg);
    assert_eq!(provider, "Codex / OpenAI");
    assert_eq!(model, "gpt-5.6-terra");
}

#[test]
fn selector_and_footer_are_consistent() {
    // Catálogo explícito: sin esto el test depende de qué modelos reporte el
    // puente en ese momento, y pasa o falla según el estado de la máquina.
    let _env = crate::ui::demo_mode::test_env_lock();
    let _catalogo = crate::app::catalog_fixture::CatalogoAislado::con(serde_json::json!({
        "providers": [{
            "id": "codex_cli",
            "display_name": "Codex / OpenAI",
            "family": "Codex / OpenAI",
            "usable_now": true,
            "models": ["gpt-5.6-terra"]
        }]
    }));
    let mut cfg = make_openai_config();
    cfg.config.active_provider_id = "codex_cli".to_string();
    cfg.config.active_alias = "gpt-5.6-terra".to_string();
    let panel = ModelPanel::from_config_with_openai_env(&cfg, None, None);
    assert!(panel.catalog_error.is_none());
    let selected = panel
        .model_choices
        .iter()
        .find(|choice| {
            choice.provider_id == cfg.config.active_provider_id
                && choice.key == cfg.config.active_alias
        })
        .unwrap();
    let footer = crate::app::runtime_identity::statusbar_identity(&cfg);
    assert_eq!(footer.0, selected.family);
    assert_eq!(footer.1, selected.key);
}

#[test]
fn provider_model_update_is_atomic() {
    let mut cfg = make_openai_config();
    let mut panel = ModelPanel::from_config_with_openai_env(&cfg, None, Some("gpt-5.4"));
    panel.active_provider_id = "mimo_code".to_string();
    panel.active_model_key = "mimo-v2.5".to_string();
    panel.apply_to_config(&mut cfg);
    assert_eq!(
        (
            cfg.config.active_provider_id.as_str(),
            cfg.config.active_alias.as_str()
        ),
        ("mimo_code", "mimo-v2.5")
    );
}

/// Bug 3: Enter en un ítem de /modelo confirma y cierra SOLO el popup —
/// no aparece ningún system note, así que view_messages sigue vacío y la
/// pantalla base sigue siendo el splash.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // test serializes env mutation via a std Mutex; keeping the guard held across await is intentional
async fn test_enter_en_modelo_no_saca_del_splash() {
        // Aislado: sin esto el test lee el catálogo de quien lo corra.
        // El candado va ANTES del guard: XDG_DATA_HOME es estado global.
        let _env = crate::ui::demo_mode::test_env_lock();
        let _catalogo = crate::app::catalog_fixture::CatalogoAislado::vacio();
    let (mut app, _) = crate::app::App::new_headless(80, 24).await;
    let tmp = tempfile::tempdir().unwrap();
    app.services.config_path_override = Some(tmp.path().join("settings.json"));
    *app.services.nexum_config.write() = make_openai_config();
    {
        let mut cfg = app.services.nexum_config.write();
        cfg.config.active_provider_id = "opencode_go".to_string();
        cfg.config.active_alias = "deepseek-v4-flash".to_string();
        cfg.config.providers[0].id = "opencode_go".to_string();
        cfg.config.providers[0].name = Some("OpenCode Go".to_string());
    }

    let cfg = app.services.nexum_config.read().clone();
    let mut panel = ModelPanel::from_config_with_openai_env(&cfg, None, Some("deepseek-v4-flash"));
    assert!(
        app.session_mgr.current().messages.view_messages.is_empty(),
        "precondición: splash (sin mensajes)"
    );

    let mut ctx = PanelContext {
        services: &mut app.services,
        session_mgr: &mut app.session_mgr,
        acp_client: None,
    };
    let result = panel.handle_key(
        tui_textarea::Input {
            key: tui_textarea::Key::Enter,
            ..Default::default()
        },
        &mut ctx,
    );
    assert!(matches!(result, EventResult::Consumed));
    assert!(
        app.session_mgr.current().messages.view_messages.is_empty(),
        "después de elegir modelo NO debe haber mensajes: el splash se mantiene"
    );
}
