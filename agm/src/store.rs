use crate::error::{AgmError, Result};
use crate::types::{PackageManifest, Resolution};
use std::path::{Path, PathBuf};

/// Store manager: ~/.agm/store/
pub struct Store {
    pub(crate) root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Get git package path in store: store/git_{owner}_{repo}@{commit}/
    pub fn git_package_path(&self, repo_url: &str, commit: &str) -> PathBuf {
        let id = sanitize_repo_id(repo_url);
        self.root
            .join(format!("git_{id}@{commit}", id = id, commit = commit))
    }

    /// Get registry package path in store: store/<name>@<version>/
    pub fn registry_package_path(&self, name: &str, version: &str) -> PathBuf {
        let safe_name = name.replace('/', "_");
        self.root.join(format!("{}@{}", safe_name, version))
    }

    /// Ensure store root directory exists
    pub fn ensure_root(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        Ok(())
    }

    /// List all package directories in store
    pub fn list_packages(&self) -> Result<Vec<PathBuf>> {
        let mut entries = Vec::new();
        if !self.root.exists() {
            return Ok(entries);
        }
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                entries.push(entry.path());
            }
        }
        Ok(entries)
    }

    /// Read agm.package.json from a package directory
    pub fn read_package_manifest(&self, package_dir: &Path) -> Result<PackageManifest> {
        let manifest_path = package_dir.join("agm.package.json");
        if !manifest_path.exists() {
            return Err(AgmError::Other(format!(
                "agm.package.json not found in {}",
                package_dir.display()
            )));
        }
        PackageManifest::load(&manifest_path)
    }

    /// Remove a package directory
    pub fn remove(&self, package_dir: &Path) -> Result<()> {
        if package_dir.exists() {
            std::fs::remove_dir_all(package_dir)?;
        }
        Ok(())
    }
}

/// Convert repo URL to a safe filesystem identifier.
///
/// For remote URLs the full path is sanitized. For local filesystem paths (used in
/// tests and local workflows) only the last two path components are used so the store
/// directory name stays short and avoids Windows MAX_PATH issues.
fn sanitize_repo_id(url: &str) -> String {
    let clean = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("git@")
        .replace("github.com/", "");

    let is_local_path = !url.contains("://")
        && (clean.contains(':') || clean.starts_with('\\') || clean.starts_with('/'));

    if is_local_path {
        let parts: Vec<&str> = clean.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
        if parts.len() >= 2 {
            return format!("{}_{}", parts[parts.len() - 2], parts[parts.len() - 1]);
        }
    }

    clean.replace(':', "/").replace(['/', '@'], "_")
}

/// Install a package from a temp directory into the store
pub fn install_to_store(
    store: &Store,
    temp_dir: &Path,
    resolution: &Resolution,
    pkg_name: &str,
    version: &str,
) -> Result<PathBuf> {
    let dest = match resolution {
        Resolution::Git { repo, commit, .. } => store.git_package_path(repo, commit),
        Resolution::Registry { .. } => store.registry_package_path(pkg_name, version),
    };

    if dest.exists() {
        return Ok(dest);
    }

    store.ensure_root()?;

    std::fs::rename(temp_dir, &dest)
        .map_err(|e| AgmError::Other(format!("failed to move package to store: {}", e)))?;

    Ok(dest)
}
