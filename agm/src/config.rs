use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgmConfig {
    #[serde(default = "default_registry")]
    pub default_registry: String,
    #[serde(default)]
    pub registry_token: Option<String>,
    #[serde(default = "default_target")]
    pub default_target: String,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default = "default_store_path")]
    pub store_path: PathBuf,
}

fn default_registry() -> String {
    "https://registry.agm.dev".into()
}
fn default_target() -> String {
    "claude".into()
}
fn default_concurrency() -> usize {
    4
}
fn default_store_path() -> PathBuf {
    agm_dir().join("store")
}

impl Default for AgmConfig {
    fn default() -> Self {
        Self {
            default_registry: default_registry(),
            registry_token: None,
            default_target: default_target(),
            concurrency: default_concurrency(),
            store_path: default_store_path(),
        }
    }
}

pub fn agm_dir() -> PathBuf {
    dirs_next::home_dir()
        .expect("cannot find home directory")
        .join(".agm")
}

pub fn config_path() -> PathBuf {
    agm_dir().join("config.json")
}

impl AgmConfig {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = agm_dir();
        std::fs::create_dir_all(&dir)?;
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(config_path(), content)?;
        Ok(())
    }

    pub fn ensure_store_dir(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.store_path)?;
        Ok(self.store_path.clone())
    }
}
