use crate::config::*;
use crate::types::DependencySpec;

#[test]
fn test_default_config() {
    let cfg = AgmConfig::default();
    assert_eq!(cfg.default_registry, "https://registry.agm.dev");
    assert_eq!(cfg.default_target, "claude");
    assert_eq!(cfg.concurrency, 4);
}

#[test]
fn test_config_roundtrip() {
    let cfg = AgmConfig::default();
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: AgmConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.default_registry, cfg.default_registry);
    assert_eq!(parsed.concurrency, cfg.concurrency);
}

#[test]
fn test_dependency_spec_simple_roundtrip() {
    let spec = DependencySpec::Simple("abc123".into());
    let json = serde_json::to_string(&spec).unwrap();
    assert_eq!(json, "\"abc123\"");
    let parsed: DependencySpec = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, spec);
}

#[test]
fn test_dependency_spec_detailed_roundtrip() {
    let spec = DependencySpec::Detailed {
        version: "^1.0.0".into(),
        base: None,
        pick: vec!["grill-*".into()],
        omit: vec!["**/*-test".into()],
    };
    let json = serde_json::to_string(&spec).unwrap();
    let parsed: DependencySpec = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, spec);
}

#[test]
fn test_project_manifest_mixed_deps() {
    use crate::types::ProjectManifest;
    use std::collections::BTreeMap;

    let mut skills = BTreeMap::new();
    skills.insert(
        "@git/owner/repo".into(),
        DependencySpec::Simple("abc123".into()),
    );
    skills.insert(
        "some-pkg".into(),
        DependencySpec::Detailed {
            version: "^1.0.0".into(),
            base: None,
            pick: vec!["interview".into()],
            omit: vec![],
        },
    );

    let manifest = ProjectManifest {
        name: "test".into(),
        version: "0.1.0".into(),
        description: String::new(),
        author: String::new(),
        registry: None,
        targets: vec!["claude".into()],
        skills,
        agents: BTreeMap::new(),
        mcp: BTreeMap::new(),
        overrides: BTreeMap::new(),
    };

    let json = serde_json::to_string_pretty(&manifest).unwrap();
    let parsed: ProjectManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, manifest);
}
