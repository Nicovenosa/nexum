#[test]
fn test_self_update_reports_local_installer_only() {
    assert_eq!(
        crate::commands::self_update::disabled_message(),
        "La actualizacion integrada todavia no esta disponible. Usa solamente el installer local canonico de Nexum."
    );
}

#[test]
fn test_self_update_uses_documented_unavailable_exit_code() {
    assert_eq!(crate::commands::self_update::DISABLED_EXIT_CODE, 69);
}
