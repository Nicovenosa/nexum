use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Result;

/// Dependency declaration: either a plain version string or a detailed object with pick/omit/base filters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DependencySpec {
    Simple(String),
    Detailed {
        version: String,
        /// Optional base directory inside the package root where discovery should start.
        /// Paths are relative to the package root and affect discovery only; pick/omit still
        /// operate on the detected glob paths relative to the package root.
        #[serde(default)]
        base: Option<String>,
        #[serde(default)]
        pick: Vec<String>,
        #[serde(default)]
        omit: Vec<String>,
    },
}

impl DependencySpec {
    pub fn version(&self) -> &str {
        match self {
            DependencySpec::Simple(v) => v,
            DependencySpec::Detailed { version, .. } => version,
        }
    }

    pub fn base(&self) -> Option<&str> {
        match self {
            DependencySpec::Simple(_) => None,
            DependencySpec::Detailed { base, .. } => base.as_deref(),
        }
    }
}

/// agm.json — project manifest
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub registry: Option<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub skills: BTreeMap<String, DependencySpec>,
    #[serde(default)]
    pub agents: BTreeMap<String, DependencySpec>,
    #[serde(default)]
    pub mcp: BTreeMap<String, DependencySpec>,
    #[serde(default)]
    pub overrides: BTreeMap<String, String>,
}

fn default_version() -> String {
    "0.1.0".into()
}

/// agm.lock.json — lock file
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockFile {
    #[serde(default = "default_lockfile_version")]
    pub lockfile_version: u32,
    pub importers: BTreeMap<String, LockImporter>,
    pub packages: BTreeMap<String, LockedPackage>,
}

fn default_lockfile_version() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockImporter {
    #[serde(default)]
    pub skills: BTreeMap<String, LockDependency>,
    #[serde(default)]
    pub agents: BTreeMap<String, LockDependency>,
    #[serde(default)]
    pub mcp: BTreeMap<String, LockDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockDependency {
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedPackage {
    pub resolution: Resolution,
    #[serde(default)]
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Resolution {
    #[serde(rename = "git")]
    Git { repo: String, commit: String },
    #[serde(rename = "registry")]
    Registry { integrity: String },
}

/// agm.package.json — package manifest
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManifest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub mcp: Vec<String>,
}

impl ProjectManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

impl LockFile {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

impl PackageManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }
}
