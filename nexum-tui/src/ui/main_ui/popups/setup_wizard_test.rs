    #[test]
    fn test_mask_api_key() {
        assert_eq!(mask_api_key(""), "");
        assert_eq!(mask_api_key("short"), "•••••");
        assert_eq!(mask_api_key("sk-ant-test-key-12345"), "sk-a••••2345");
    }

    async fn render_headless(
        wizard: SetupWizardPanel,
    ) -> (App, crate::ui::headless::HeadlessHandle) {
        let (mut app, mut handle) = App::new_headless(120, 30).await;
        app.global_ui.setup_wizard = Some(wizard);
        handle
            .terminal
            .draw(|f| crate::ui::main_ui::render(f, &mut app))
            .unwrap();
        (app, handle)
    }

    #[tokio::test]
    async fn test_render_step_choose() {
        let mut wizard = SetupWizardPanel::new();
        wizard.step = SetupStep::Choose;
        let (_, handle) = render_headless(wizard).await;
        assert!(handle.contains("Custom API"));
        assert!(handle.contains("Import existing config"));
    }

    #[tokio::test]
    async fn test_render_step_form() {
        let mut wizard = SetupWizardPanel::new();
        wizard.step = SetupStep::Form;
        let (_, handle) = render_headless(wizard).await;
        assert!(handle.contains("Configure"));
        assert!(handle.contains("Submit"));
    }

    #[tokio::test]
    async fn test_render_done_page() {
        let mut wizard = SetupWizardPanel::new();
        wizard.step = SetupStep::Done;
        wizard.providers[0].field_api_key.set_value("sk-ant-test1234xyz");
        let (_, handle) = render_headless(wizard).await;
        // Antes assertaba "Complete", el título viejo con su decoración
        // "── Setup Nexum ──" embebida. Ahora la identidad la pone el chrome y
        // el título es sólo el título: se asserta lo que el rediseño garantiza.
        assert!(handle.contains("Done"), "el título de la pantalla final");
    }

    // ─── El rediseño ──────────────────────────────────────────────────────
    //
    // Lo que estos tests fijan es que el wizard PAREZCA Nexum: que la marca
    // esté en todas las pantallas y que no queden glifos que la terminal no
    // pueda dibujar. La queja original fue "hoy no parece Nexum".

    #[tokio::test]
    async fn todas_las_pantallas_llevan_la_marca() {
        for step in [
            SetupStep::Language,
            SetupStep::Choose,
            SetupStep::Form,
            SetupStep::Done,
        ] {
            let mut wizard = SetupWizardPanel::new();
            wizard.step = step;
            let (_, handle) = render_headless(wizard).await;
            assert!(
                handle.contains("NEXUM") || handle.contains("█"),
                "falta la marca en {step:?}: el wizard tiene que parecer Nexum"
            );
        }
    }

    #[tokio::test]
    async fn ninguna_pantalla_dibuja_glifos_que_la_terminal_no_tiene() {
        // Los "cuadraditos" del menú de idioma eran esto. Vale como regla
        // general de la TUI, no sólo para el wizard.
        for step in [
            SetupStep::Language,
            SetupStep::Choose,
            SetupStep::Form,
            SetupStep::Done,
        ] {
            let mut wizard = SetupWizardPanel::new();
            wizard.step = step;
            let (_, handle) = render_headless(wizard).await;
            let pantalla = handle.snapshot().join("\n");
            for glifo in ['❯', '✓', '●', '⚠', '◇', '✗'] {
                assert!(
                    !pantalla.contains(glifo),
                    "glifo {glifo:?} en {step:?}: usar ASCII"
                );
            }
        }
    }

    #[tokio::test]
    async fn la_pantalla_de_idioma_ofrece_un_nombre_legible_para_cada_opcion() {
        // `中文` a secas sale como cuadraditos sin fuente CJK, y ésta es
        // justo la pantalla donde una opción ilegible es más cara.
        let mut wizard = SetupWizardPanel::new();
        wizard.step = SetupStep::Language;
        let (_, handle) = render_headless(wizard).await;
        assert!(handle.contains("English"));
        assert!(handle.contains("Chinese"));
        assert!(handle.contains("Espanol"));
    }

    /// Vuelca las pantallas del wizard para inspección visual.
    /// `cargo test -p nexum-tui vistazo_del_wizard -- --nocapture --ignored`
    #[tokio::test]
    #[ignore = "herramienta de inspección visual, no aserción"]
    async fn vistazo_del_wizard() {
        let (ancho, alto) = match std::env::var("VISTAZO").as_deref() {
            Ok("chico") => (80u16, 24u16),
            _ => (120, 30),
        };
        for step in [SetupStep::Language, SetupStep::Choose, SetupStep::Done] {
            let mut wizard = SetupWizardPanel::new();
            wizard.step = step;
            let (mut app, mut handle) = App::new_headless(ancho, alto).await;
            app.global_ui.setup_wizard = Some(wizard);
            handle
                .terminal
                .draw(|f| crate::ui::main_ui::render(f, &mut app))
                .unwrap();
            println!("\n╔══ {step:?} ══════════════════════════════════");
            for l in handle.snapshot() {
                println!("║{}", l.trim_end());
            }
        }
    }
