use crate::store::*;
use crate::types::*;
use std::path::PathBuf;

#[test]
fn test_manifest_parses_detailed_dependency() {
    use crate::types::{DependencySpec, ProjectManifest};

    let json = r#"{
        "name": "test",
        "skills": {
            "some-pkg": {
                "version": "^1.0.0",
                "pick": ["interview", "grill-*"],
                "omit": ["**/*-test"]
            }
        }
    }"#;

    let manifest: ProjectManifest = serde_json::from_str(json).unwrap();
    let spec = manifest.skills.get("some-pkg").unwrap();
    assert!(matches!(spec, DependencySpec::Detailed { .. }));
    assert_eq!(spec.version(), "^1.0.0");
}

#[test]
fn test_git_package_path() {
    let store = Store::new(PathBuf::from("/tmp/agm-store"));
    let path = store.git_package_path("https://github.com/user/repo", "abc123def456");
    assert_eq!(
        path,
        PathBuf::from("/tmp/agm-store/git_user_repo@abc123def456")
    );
}

#[test]
fn test_registry_package_path() {
    let store = Store::new(PathBuf::from("/tmp/agm-store"));
    let path = store.registry_package_path("scope/pkg", "1.2.3");
    assert_eq!(path, PathBuf::from("/tmp/agm-store/scope_pkg@1.2.3"));
}

#[test]
fn test_ensure_root_creates_dir() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Store::new(tmp.path().join("store"));
    assert!(!store.root.exists());
    store.ensure_root().unwrap();
    assert!(store.root.exists());
}

#[test]
fn test_install_to_store_git() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Store::new(tmp.path().join("store"));
    let src = tmp.path().join("pkg");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("SKILL.md"), "# Skill").unwrap();

    let resolution = Resolution::Git {
        repo: "https://github.com/foo/bar".into(),
        commit: "abc123".into(),
    };

    let dest = install_to_store(&store, &src, &resolution, "@git/foo/bar", "abc123").unwrap();
    assert!(dest.exists());
    assert!(dest.join("SKILL.md").exists());
}

#[test]
fn test_list_packages() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Store::new(tmp.path().join("store"));
    store.ensure_root().unwrap();
    std::fs::create_dir(store.root.join("pkg_a@1.0.0")).unwrap();
    std::fs::create_dir(store.root.join("pkg_b@2.0.0")).unwrap();

    let pkgs = store.list_packages().unwrap();
    assert_eq!(pkgs.len(), 2);
}

#[test]
fn test_git_package_path_uses_short_id_for_local_path() {
    let store = Store::new(PathBuf::from("/tmp/agm-store"));
    let path = store.git_package_path(
        r"C:\Users\runner\AppData\Local\Temp\.tmpABC123\owner/repo",
        "abc123def456",
    );
    assert_eq!(
        path,
        PathBuf::from("/tmp/agm-store/git_owner_repo@abc123def456")
    );
}
