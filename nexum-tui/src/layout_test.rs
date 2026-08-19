use std::path::Path;

use super::InstalledLayoutV1;

fn write_file(path: &Path) {
    std::fs::write(path, "test").unwrap();
}

fn make_layout(root: &Path) {
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
        write_file(&root.join(name));
    }
}

#[test]
fn installed_layout_accepts_direct_version_executable() {
    let tmp = tempfile::tempdir().unwrap();
    let version = tmp.path().join("lib/nexum/v1");
    make_layout(&version);
    // macOS /var/folders canonicalizes to /private/var/folders; normalize the
    // expected side so the comparison is platform-neutral.
    let version = std::fs::canonicalize(&version).unwrap_or(version);

    let layout = InstalledLayoutV1::from_executable(&version.join("nexum")).unwrap();

    assert_eq!(layout.version_root(), version);
    assert_eq!(layout.base_catalog(), version.join(nexum_acp::provider::catalog_path::PACKAGED_BASE_FILE_NAME));
    assert_eq!(layout.provider_package(), version.join("src/nexum_providers"));
}

#[cfg(unix)]
#[test]
fn installed_layout_canonicalizes_current_and_prefix_launcher_symlinks() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let version = tmp.path().join("lib/nexum/v1");
    make_layout(&version);
    // macOS /var/folders canonicalizes to /private/var/folders; normalize the
    // expected side so the comparison is platform-neutral.
    let version = std::fs::canonicalize(&version).unwrap_or(version);
    let current = tmp.path().join("lib/nexum/current");
    let launcher_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&launcher_dir).unwrap();
    symlink("v1", &current).unwrap();
    symlink("../lib/nexum/current/nexum", launcher_dir.join("nexum")).unwrap();

    let layout = InstalledLayoutV1::from_executable(&launcher_dir.join("nexum")).unwrap();

    assert_eq!(layout.version_root(), version);
    assert_eq!(layout.acp_host(), version.join("nexum-acp-host"));
    assert_eq!(layout.reconcile(), version.join("nexum-autologin-reconcile"));
}

#[test]
fn installed_layout_rejects_incomplete_or_checkout_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = tmp.path().join("checkout");
    std::fs::create_dir_all(checkout.join("src/nexum_providers")).unwrap();
    write_file(&checkout.join("nexum"));

    assert!(InstalledLayoutV1::from_executable(&checkout.join("nexum")).is_none());
}

#[cfg(unix)]
#[test]
fn installed_layout_rejects_resource_symlink_escaping_version_root() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let version = tmp.path().join("version");
    let checkout = tmp.path().join("checkout/src/nexum_providers");
    make_layout(&version);
    std::fs::remove_dir(version.join("src/nexum_providers")).unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    symlink(&checkout, version.join("src/nexum_providers")).unwrap();

    assert!(InstalledLayoutV1::from_executable(&version.join("nexum")).is_none());
}
