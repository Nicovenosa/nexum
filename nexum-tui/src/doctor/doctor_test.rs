// ALLOW justificado: temporal de test con PID en el nombre. No hay recurso compartido entre
// procesos, que es lo que el lint protege.
#![allow(clippy::disallowed_methods)]

use super::*;

fn nexum_identity() -> checks::ProductIdentity<'static> {
    checks::ProductIdentity {
        product_name: "Nexum",
        product_id: "nexum",
        branding: "Nexum",
        launcher: "nexum",
    }
}

#[test]
fn test_identity_allows_supported_opencode_providers_not_configured() {
    let catalog = serde_json::json!({
        "providers": [
            {"id": "opencode", "status": "not_configured", "usable_now": false},
            {"id": "opencode_zen", "status": "not_configured", "usable_now": false},
            {"id": "opencode_go", "status": "not_configured", "usable_now": false}
        ]
    });
    assert_eq!(catalog["providers"].as_array().unwrap().len(), 3);
    assert_eq!(
        checks::product_identity_status(nexum_identity()),
        Status::Pass
    );
}

#[test]
fn supported_opencode_provider_does_not_change_nexum_product_identity() {
    let catalog = serde_json::json!({
        "providers": [
            {"id": "opencode", "status": "configured", "usable_now": true},
            {"id": "opencode_zen", "status": "not_configured", "usable_now": false}
        ]
    });
    assert_eq!(catalog["providers"].as_array().unwrap().len(), 2);
    assert_eq!(
        checks::product_identity_status(nexum_identity()),
        Status::Pass
    );
}

#[test]
fn test_identity_allows_detected_and_usable_opencode_providers() {
    let catalog = serde_json::json!({
        "providers": [
            {"id": "opencode", "native_login_detected": true, "usable_now": false},
            {"id": "opencode_zen", "usable_now": true}
        ]
    });
    assert!(catalog["providers"][0]["native_login_detected"]
        .as_bool()
        .unwrap());
    assert!(catalog["providers"][1]["usable_now"].as_bool().unwrap());
    assert_eq!(
        checks::product_identity_status(nexum_identity()),
        Status::Pass
    );
}

#[test]
fn test_identity_allows_opencode_models_regardless_of_selectability() {
    let catalog = serde_json::json!({
        "providers": [{
            "id": "opencode_go",
            "usable_now": true,
            "models": ["opencode-visible", "opencode-selectable"]
        }]
    });
    assert_eq!(
        catalog["providers"][0]["models"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        checks::product_identity_status(nexum_identity()),
        Status::Pass
    );
}

#[test]
fn test_identity_rejects_opencode_product_name() {
    let mut identity = nexum_identity();
    identity.product_name = "OpenCode";
    assert_eq!(checks::product_identity_status(identity), Status::Fail);
}

#[test]
fn test_identity_rejects_opencode_launcher_or_branding() {
    let mut launcher = nexum_identity();
    launcher.launcher = "opencode";
    assert_eq!(checks::product_identity_status(launcher), Status::Fail);
    let mut branding = nexum_identity();
    branding.branding = "OpenCode terminal";
    assert_eq!(checks::product_identity_status(branding), Status::Fail);
}

#[test]
fn test_identity_allows_normal_nexum_branding_without_opencode_providers() {
    assert_eq!(
        checks::product_identity_status(nexum_identity()),
        Status::Pass
    );
}

/// Regresión P0 de la línea B: cubierta también por los dos tests de arriba,
/// pero se conserva con su nombre porque es la que nombra el incidente.
#[test]
fn opencode_product_identity_still_fails_doctor() {
    let mut product = nexum_identity();
    product.product_name = "OpenCode";
    assert_eq!(checks::product_identity_status(product), Status::Fail);

    let mut launcher = nexum_identity();
    launcher.launcher = "opencode";
    assert_eq!(checks::product_identity_status(launcher), Status::Fail);
}

#[test]
fn test_status_glyphs_distinct() {
    let all = [
        Status::Pass,
        Status::Warn,
        Status::Fail,
        Status::Skip,
        Status::Unknown,
    ];
    let glyphs: std::collections::HashSet<_> = all.iter().map(|s| s.glyph()).collect();
    assert_eq!(glyphs.len(), 5, "cada estado tiene glyph único");
}

#[test]
fn test_check_result_builder() {
    let r = CheckResult::new("X-1", Status::Warn, "desc", "ev").rec("hacé algo");
    assert_eq!(r.id, "X-1");
    assert_eq!(r.status, Status::Warn);
    assert_eq!(r.recommendation.as_deref(), Some("hacé algo"));
    assert!(r.fixable.is_none());
}

#[test]
fn test_fixable_check_carries_closure() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let applied = Arc::new(AtomicBool::new(false));
    let a = applied.clone();
    let r = CheckResult::new("X-2", Status::Fail, "d", "e").with_fix("repara X", move || {
        a.store(true, Ordering::SeqCst);
        Ok(())
    });
    let fix = r.fixable.expect("fixable presente");
    assert_eq!(fix.describe, "repara X");
    (fix.apply)().unwrap();
    assert!(applied.load(Ordering::SeqCst), "el fix se aplicó");
}

/// El doctor corre completo sin panic en un entorno aislado y no expone secretos.
#[test]
fn test_run_no_panic_no_secrets() {
    // Setea XDG_DATA_HOME, que es estado global: sin el candado pisa a los tests
    // que aíslan el catálogo de providers.
    let _guard = crate::ui::demo_mode::test_env_lock();
    let d = std::env::temp_dir().join(format!("doctor-test-{}", std::process::id()));
    std::fs::create_dir_all(d.join(".secrets")).unwrap();
    // secret canario con permisos inseguros
    std::fs::write(d.join(".secrets/canary.env"), "SECRET=sk-canary-doctor-xyz").unwrap();
    std::env::set_var("NEXUM_CLI_DIR", &d);
    std::env::set_var("XDG_CACHE_HOME", d.join("cache"));
    std::env::set_var("XDG_CONFIG_HOME", d.join("cfg"));
    std::env::set_var("XDG_DATA_HOME", d.join("data"));
    let ctx = DoctorCtx::detect();
    let mut results = Vec::new();
    checks::runtime(&ctx, &mut results);
    checks::hardware(&mut results);
    checks::config(&mut results);
    checks::security(&ctx, &mut results);
    checks::identity(&ctx, &mut results);
    checks::network(&mut results);
    assert!(!results.is_empty());
    // ningún evidence/recommendation contiene el valor del secreto
    for r in &results {
        assert!(
            !r.evidence.contains("sk-canary-doctor-xyz"),
            "doctor no expone secretos"
        );
        if let Some(rec) = &r.recommendation {
            assert!(!rec.contains("sk-canary-doctor-xyz"));
        }
    }
    std::env::remove_var("NEXUM_CLI_DIR");
    std::fs::remove_dir_all(&d).ok();
}

fn write_layout_file(root: &std::path::Path, name: &str) {
    std::fs::write(root.join(name), "test").unwrap();
}

fn make_installed_layout(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src/nexum_providers")).unwrap();
    std::fs::create_dir_all(root.join("schemas")).unwrap();
    for name in [
        "nexum",
        "nexum-acp-host",
        "nexum-autologin-reconcile",
        nexum_acp::provider::catalog_path::INSTALLED_BASE_FILE_NAME,
        "provider-catalog-base.json",
        "reserved-models.json",
        "LICENSE",
        "NOTICE",
        "PACKAGE_MANIFEST.json",
    ] {
        write_layout_file(root, name);
    }
}

fn provider_independence_status(root: &std::path::Path) -> Status {
    let layout = crate::layout::InstalledLayoutV1::from_executable(&root.join("nexum"));
    checks::provider_checkout_independence(layout).status
}

#[test]
fn test_provider_checkout_independence_passes_only_complete_installed_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let version = tmp.path().join("version");
    make_installed_layout(&version);
    std::fs::create_dir_all(tmp.path().join("checkout/src/nexum_providers")).unwrap();

    assert_eq!(provider_independence_status(&version), Status::Pass);
}

#[test]
fn test_provider_checkout_independence_fails_each_missing_required_resource() {
    for missing in [
        nexum_acp::provider::catalog_path::INSTALLED_BASE_FILE_NAME,
        "provider-catalog-base.json",
        "reserved-models.json",
        "nexum-acp-host",
        "nexum-autologin-reconcile",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let version = tmp.path().join("version");
        make_installed_layout(&version);
        std::fs::remove_file(version.join(missing)).unwrap();
        assert_eq!(
            provider_independence_status(&version),
            Status::Fail,
            "missing {missing} must fail"
        );
    }

    let tmp = tempfile::tempdir().unwrap();
    let version = tmp.path().join("version");
    make_installed_layout(&version);
    std::fs::remove_dir_all(version.join("src/nexum_providers")).unwrap();
    assert_eq!(provider_independence_status(&version), Status::Fail);
}

#[cfg(unix)]
#[test]
fn test_provider_checkout_independence_accepts_direct_and_launcher_layouts() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let version = tmp.path().join("lib/nexum/v1");
    make_installed_layout(&version);
    let launcher_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&launcher_dir).unwrap();
    symlink("v1", tmp.path().join("lib/nexum/current")).unwrap();
    symlink("../lib/nexum/current/nexum", launcher_dir.join("nexum")).unwrap();

    let direct = crate::layout::InstalledLayoutV1::from_executable(&version.join("nexum"));
    let launcher = crate::layout::InstalledLayoutV1::from_executable(&launcher_dir.join("nexum"));
    assert_eq!(checks::provider_checkout_independence(direct).status, Status::Pass);
    assert_eq!(checks::provider_checkout_independence(launcher).status, Status::Pass);
}

#[test]
fn test_provider_checkout_independence_rejects_checkout_only_source_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = tmp.path().join("checkout");
    std::fs::create_dir_all(checkout.join("src/nexum_providers")).unwrap();
    write_layout_file(&checkout, "nexum");

    assert_eq!(provider_independence_status(&checkout), Status::Fail);
}

#[cfg(unix)]
#[test]
fn test_provider_checkout_independence_rejects_resource_symlink_to_checkout() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let version = tmp.path().join("version");
    let checkout = tmp.path().join("checkout/src/nexum_providers");
    make_installed_layout(&version);
    std::fs::remove_dir(version.join("src/nexum_providers")).unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    symlink(&checkout, version.join("src/nexum_providers")).unwrap();

    assert_eq!(provider_independence_status(&version), Status::Fail);
}
