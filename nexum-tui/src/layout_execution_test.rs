use std::path::Path;

use crate::layout::InstalledLayoutV1;

fn make_installed_layout(root: &Path) {
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
        std::fs::write(root.join(name), "test").unwrap();
    }
}

#[cfg(unix)]
#[test]
fn direct_version_and_prefix_launcher_resolve_identical_resources() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let version = tmp.path().join("lib/nexum/v1");
    make_installed_layout(&version);
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    symlink("v1", tmp.path().join("lib/nexum/current")).unwrap();
    symlink("../lib/nexum/current/nexum", bin.join("nexum")).unwrap();

    let direct = InstalledLayoutV1::from_executable(&version.join("nexum")).unwrap();
    let launched = InstalledLayoutV1::from_executable(&bin.join("nexum")).unwrap();

    assert_eq!(direct.version_root(), launched.version_root());
    assert_eq!(direct.catalog_output(), launched.catalog_output());
    assert_eq!(direct.base_catalog(), launched.base_catalog());
    assert_eq!(direct.reserved_models(), launched.reserved_models());
    assert_eq!(direct.provider_package(), launched.provider_package());
    assert_eq!(direct.schemas(), launched.schemas());
    assert_eq!(direct.acp_host(), launched.acp_host());
    assert_eq!(direct.reconcile(), launched.reconcile());
}
