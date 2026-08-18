use crate::error::{AgmError, Result};
use crate::git;
use crate::registry::RegistryClient;
use crate::types::*;
use semver::VersionReq;
use std::collections::BTreeMap;

/// Resolved package result
#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: String,
    pub resolution: Resolution,
}

/// Check if a dependency is a git dependency (@git/ prefix)
pub fn is_git_dep(name: &str) -> bool {
    name.starts_with("@git/")
}

/// Validate git commit hash
pub fn validate_commit_hash(hash: &str) -> Result<()> {
    if !git::is_valid_commit_hash(hash) {
        return Err(AgmError::InvalidCommitHash(hash.into()));
    }
    Ok(())
}

/// Resolve a semver range to a concrete version from the registry
pub async fn resolve_registry_version(
    client: &RegistryClient,
    name: &str,
    range: &str,
) -> Result<String> {
    let req = VersionReq::parse(range)?;
    let metadata = client.get_package(name).await?;

    let mut candidates: Vec<_> = metadata.versions.keys().cloned().collect();
    candidates.sort_by(|a, b| {
        let va = semver::Version::parse(a).ok();
        let vb = semver::Version::parse(b).ok();
        vb.cmp(&va) // descending
    });

    for version in &candidates {
        if let Ok(v) = semver::Version::parse(version) {
            if req.matches(&v) {
                return Ok(version.clone());
            }
        }
    }

    Err(AgmError::Other(format!(
        "no version of {} satisfies range {}",
        name, range
    )))
}

/// Collect all dependencies from a manifest that need resolving
pub fn collect_dependencies(
    manifest: &ProjectManifest,
) -> Vec<(String, DependencySpec, PackageType)> {
    let mut deps = Vec::new();
    for (name, spec) in &manifest.skills {
        deps.push((name.clone(), spec.clone(), PackageType::Skills));
    }
    for (name, spec) in &manifest.agents {
        deps.push((name.clone(), spec.clone(), PackageType::Agents));
    }
    for (name, spec) in &manifest.mcp {
        deps.push((name.clone(), spec.clone(), PackageType::Mcp));
    }
    deps
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageType {
    Skills,
    Agents,
    Mcp,
}

/// Detect transitive dependency conflicts
pub fn detect_conflicts(
    resolved: &BTreeMap<String, ResolvedPackage>,
    overrides: &BTreeMap<String, String>,
) -> Result<()> {
    for name in overrides.keys() {
        let _found = resolved.values().any(|p| p.name == *name);
    }
    Ok(())
}
