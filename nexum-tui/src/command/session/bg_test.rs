    async fn headless_app() -> App {
        App::new_headless(80, 24).await.0
    }

    #[tokio::test]
    async fn test_bg_cmd_empty_args_shows_usage() {
        let mut app = headless_app().await;
        let cmd = BgCommand;
        cmd.execute(&mut app, "");
        assert_eq!(
            app.session_mgr.current_mut()
                .messages
                .view_messages
                .len(),
            1,
            "空参数应显示用法提示"
        );
        let text = format!(
            "{:?}",
            app.session_mgr.current_mut()
                .messages
                .view_messages[0]
        );
        // 默认 locale 可能是 en/zh-CN，两种文案都接受
        assert!(
            text.contains("用法") || text.contains("Usage"),
            "空参数应显示用法提示，实际: {}",
            text
        );
    }

    #[tokio::test]
    async fn test_bg_cmd_empty_whitespace_shows_usage() {
        let mut app = headless_app().await;
        let cmd = BgCommand;
        cmd.execute(&mut app, "   ");
        assert_eq!(
            app.session_mgr.current_mut()
                .messages
                .view_messages
                .len(),
            1,
            "纯空格参数应显示用法提示"
        );
        let text = format!(
            "{:?}",
            app.session_mgr.current_mut()
                .messages
                .view_messages[0]
        );
        assert!(
            text.contains("用法") || text.contains("Usage"),
            "纯空格参数应显示用法提示"
        );
    }

    #[test]
    fn test_bg_cmd_name() {
        let cmd = BgCommand;
        assert_eq!(cmd.name(), "bg");
    }

    #[test]
    fn test_bg_cmd_aliases() {
        let cmd = BgCommand;
        assert!(cmd.aliases().contains(&"background"));
    }

    #[test]
    fn test_bg_cmd_description_not_empty() {
        let cmd = BgCommand;
        let lc = crate::i18n::LcRegistry::default();
        assert!(!cmd.description(&lc).is_empty());
    }
