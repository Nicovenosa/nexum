use crate::types::*;
use std::collections::BTreeMap;

#[test]
fn test_parse_project_manifest() {
    let json = r#"{
        "name": "my-agent-project",
        "version": "1.0.0",
        "skills": {
            "@git/konghayao/peri/blog-writer": "abc123def456",
            "some-pkg": "^1.2.3"
        },
        "agents": {},
        "mcp": {}
    }"#;
    let manifest: ProjectManifest = serde_json::from_str(json).unwrap();
    assert_eq!(manifest.name, "my-agent-project");
    assert_eq!(manifest.skills.len(), 2);
    assert_eq!(
        manifest.skills["@git/konghayao/peri/blog-writer"],
        DependencySpec::Simple("abc123def456".into())
    );
    assert_eq!(
        manifest.skills["some-pkg"],
        DependencySpec::Simple("^1.2.3".into())
    );
}

#[test]
fn test_parse_lock_file() {
    let json = r#"{
        "lockfileVersion": 2,
        "importers": {
            ".": {
                "skills": {
                    "@git/konghayao/peri/blog-writer": { "version": "abc123def456" }
                }
            }
        },
        "packages": {
            "@git/konghayao/peri/blog-writer@abc123def456": {
                "resolution": {
                    "type": "git",
                    "repo": "https://github.com/konghayao/peri",
                    "commit": "abc123def456"
                },
                "targets": ["claude"]
            }
        }
    }"#;
    let lock: LockFile = serde_json::from_str(json).unwrap();
    assert_eq!(lock.lockfile_version, 2);
    let pkg = &lock.packages["@git/konghayao/peri/blog-writer@abc123def456"];
    assert_eq!(pkg.targets, vec!["claude"]);
    match &pkg.resolution {
        Resolution::Git { repo, commit } => {
            assert_eq!(repo, "https://github.com/konghayao/peri");
            assert_eq!(commit, "abc123def456");
        }
        _ => panic!("expected git resolution"),
    }
}

#[test]
fn test_parse_package_manifest() {
    let json = r#"{
        "name": "@git/konghayao/peri/blog-writer",
        "version": "1.0.0",
        "skills": ["export/skills/blog-writer/SKILL.md"],
        "agents": [],
        "mcp": []
    }"#;
    let pkg: PackageManifest = serde_json::from_str(json).unwrap();
    assert_eq!(pkg.name, "@git/konghayao/peri/blog-writer");
    assert_eq!(pkg.skills, vec!["export/skills/blog-writer/SKILL.md"]);
}

#[test]
fn test_project_manifest_roundtrip() {
    let manifest = ProjectManifest {
        name: "test".into(),
        version: "1.0.0".into(),
        description: String::new(),
        author: String::new(),
        registry: None,
        targets: vec![],
        skills: [("pkg".into(), DependencySpec::Simple("^1.0.0".into()))].into(),
        agents: BTreeMap::new(),
        mcp: BTreeMap::new(),
        overrides: BTreeMap::new(),
    };
    let json = serde_json::to_string(&manifest).unwrap();
    let parsed: ProjectManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "test");
    assert_eq!(
        parsed.skills["pkg"],
        DependencySpec::Simple("^1.0.0".into())
    );
}

#[test]
fn test_lock_file_roundtrip() {
    let lock = LockFile {
        lockfile_version: 1,
        importers: [(
            ".".into(),
            LockImporter {
                skills: [(
                    "pkg".into(),
                    LockDependency {
                        version: "1.0.0".into(),
                    },
                )]
                .into(),
                agents: BTreeMap::new(),
                mcp: BTreeMap::new(),
            },
        )]
        .into(),
        packages: [(
            "pkg@1.0.0".into(),
            LockedPackage {
                resolution: Resolution::Registry {
                    integrity: "sha256-abc".into(),
                },
                targets: vec!["claude".into()],
            },
        )]
        .into(),
    };
    let json = serde_json::to_string(&lock).unwrap();
    let parsed: LockFile = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.lockfile_version, 1);
    assert_eq!(parsed.packages["pkg@1.0.0"].targets, vec!["claude"]);
}
