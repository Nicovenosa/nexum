#[test]
fn test_update_reports_local_installer_only() {
    assert_eq!(
        crate::update::unavailable_message(),
        "La actualizacion integrada todavia no esta disponible. Usa solamente el installer local canonico de Nexum."
    );
}
