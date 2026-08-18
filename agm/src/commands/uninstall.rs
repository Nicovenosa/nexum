use crate::adapter::{get_adapter, symlink_name};
use crate::config::AgmConfig;
use crate::error::{AgmError, Result};
use crate::filter::{detect_package_items, filter_items};
use crate::resolver::PackageType;
use crate::store::Store;
use crate::types::*;
use std::path::PathBuf;

pub fn execute(package: &str, target: &str, project_dir: Option<PathBuf>) -> Result<()> {
    let dir = project_dir.unwrap_or_else(|| PathBuf::from("."));
    let manifest_path = dir.join("agm.json");

    if !manifest_path.exists() {
        return Err(AgmError::ManifestNotFound);
    }

    let adapter = get_adapter(target)
        .ok_or_else(|| AgmError::Other(format!("unknown target: {}", target)))?;

    let mut manifest = ProjectManifest::load(&manifest_path)?;

    // Capture the dependency spec before removing it from the manifest.
    let spec = manifest
        .skills
        .get(package)
        .or_else(|| manifest.agents.get(package))
        .or_else(|| manifest.mcp.get(package))
        .cloned();

    let removed_skills = manifest.skills.remove(package);
    let removed_agents = manifest.agents.remove(package);
    let removed_mcp = manifest.mcp.remove(package);

    if removed_skills.is_none() && removed_agents.is_none() && removed_mcp.is_none() {
        return Err(AgmError::PackageNotInManifest(package.into()));
    }

    // Try to locate the package in the store via the lock file.
    let lock_path = dir.join("agm.lock.json");
    let store_path: Option<PathBuf> = if lock_path.exists() {
        let lock = LockFile::load(&lock_path)?;
        lock.importers.get(".").and_then(|importer| {
            let version = removed_skills
                .as_ref()
                .and_then(|_| importer.skills.get(package).map(|d| d.version.clone()))
                .or_else(|| {
                    removed_agents
                        .as_ref()
                        .and_then(|_| importer.agents.get(package).map(|d| d.version.clone()))
                })
                .or_else(|| {
                    removed_mcp
                        .as_ref()
                        .and_then(|_| importer.mcp.get(package).map(|d| d.version.clone()))
                })?;
            let pkg_key = format!("{}@{}", package, version);
            let locked_pkg = lock.packages.get(&pkg_key)?;
            let config = AgmConfig::load().unwrap_or_default();
            let store = Store::new(config.store_path);
            Some(match &locked_pkg.resolution {
                Resolution::Git { repo, commit } => store.git_package_path(repo, commit),
                Resolution::Registry { .. } => store.registry_package_path(package, &version),
            })
        })
    } else {
        None
    };

    // Compute the item symlinks that were actually created for each declared type.
    let mut skill_items: Vec<String> = Vec::new();
    let mut agent_items: Vec<String> = Vec::new();
    let mut mcp_items: Vec<String> = Vec::new();

    if let (Some(spec), Some(ref store_path)) = (&spec, &store_path) {
        if removed_skills.is_some() {
            let detected =
                detect_package_items(store_path, PackageType::Skills, package, spec.base())?;
            skill_items = filter_items(&detected, spec)?
                .into_iter()
                .map(|(name, _)| name)
                .collect();
        }
        if removed_agents.is_some() {
            let detected =
                detect_package_items(store_path, PackageType::Agents, package, spec.base())?;
            agent_items = filter_items(&detected, spec)?
                .into_iter()
                .map(|(name, _)| name)
                .collect();
        }
        if removed_mcp.is_some() {
            let detected =
                detect_package_items(store_path, PackageType::Mcp, package, spec.base())?;
            mcp_items = filter_items(&detected, spec)?
                .into_iter()
                .map(|(name, _)| name)
                .collect();
        }
    }

    // Remove the per-item symlinks, plus the legacy package-name symlink.
    if removed_skills.is_some() {
        let target_dir = adapter.map_dir(PackageType::Skills, &dir);
        for item in &skill_items {
            adapter.uninstall(&target_dir, &symlink_name(item, &[]))?;
        }
        adapter.uninstall(&target_dir, &symlink_name(package, &[]))?;
    }
    if removed_agents.is_some() {
        let target_dir = adapter.map_dir(PackageType::Agents, &dir);
        for item in &agent_items {
            adapter.uninstall(&target_dir, &symlink_name(item, &[]))?;
        }
        adapter.uninstall(&target_dir, &symlink_name(package, &[]))?;
    }
    if removed_mcp.is_some() {
        let target_dir = adapter.map_dir(PackageType::Mcp, &dir);
        for item in &mcp_items {
            adapter.uninstall(&target_dir, &symlink_name(item, &[]))?;
        }
        adapter.uninstall(&target_dir, &symlink_name(package, &[]))?;
    }

    manifest.save(&manifest_path)?;

    if lock_path.exists() {
        let mut lock = LockFile::load(&lock_path)?;
        if let Some(importer) = lock.importers.get_mut(".") {
            importer.skills.remove(package);
            importer.agents.remove(package);
            importer.mcp.remove(package);
        }
        lock.packages
            .retain(|k, _| !k.starts_with(&format!("{}@", package)));
        lock.save(&lock_path)?;
    }

    println!("Uninstalled {}", package);
    Ok(())
}
