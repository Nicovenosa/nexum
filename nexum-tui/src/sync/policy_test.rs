#[test]
fn test_sync_default_is_disabled_without_server() {
    let result = super::policy::require_explicit_server(None, false);

    assert!(result.is_err());
}

#[test]
fn test_sync_requires_consent_for_explicit_server() {
    let result = super::policy::require_explicit_server(Some("wss://example.invalid"), false);

    assert!(result.is_err());
}

#[test]
fn test_sync_allows_explicit_server_after_consent() {
    let result = super::policy::require_explicit_server(Some("wss://example.invalid"), true);

    assert_eq!(result.unwrap(), "wss://example.invalid");
}
