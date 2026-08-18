    #[test]
    fn test_oauth_prompt_new() {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let prompt = OAuthPrompt::new("test-server".into(), "http://example.com/auth".into(), tx);
        assert!(prompt.field.is_empty());
        assert!(prompt.error_message.is_none());
        assert_eq!(prompt.server_name, "test-server");
    }

    #[test]
    fn test_oauth_prompt_submit_valid_url() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut prompt = OAuthPrompt::new("srv".into(), "http://auth.example.com".into(), tx);
        prompt.field.set_value("http://localhost:12345/callback?code=abc&state=xyz");
        assert!(prompt.submit());
        let result = rx.blocking_recv().unwrap();
        assert_eq!(result.code, "abc");
        assert_eq!(result.state, "xyz");
    }

    #[test]
    fn test_oauth_prompt_submit_full_url() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut prompt = OAuthPrompt::new("srv".into(), "http://auth.example.com".into(), tx);
        prompt.field.set_value("http://localhost:9999/callback?code=test_code&state=test_state");
        assert!(prompt.submit());
        let result = rx.blocking_recv().unwrap();
        assert_eq!(result.code, "test_code");
        assert_eq!(result.state, "test_state");
    }

    #[test]
    fn test_oauth_prompt_submit_invalid_url() {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let mut prompt = OAuthPrompt::new("srv".into(), "http://auth.example.com".into(), tx);
        prompt.field.set_value("not a valid url");
        assert!(!prompt.submit());
        assert!(prompt.error_message.is_some());
    }

    #[test]
    fn test_oauth_prompt_submit_empty() {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let mut prompt = OAuthPrompt::new("srv".into(), "http://auth.example.com".into(), tx);
        prompt.field.clear();
        assert!(!prompt.submit());
        assert!(prompt.error_message.is_some());
    }
