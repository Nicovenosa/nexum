use crate::error::{AgmError, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    pub name: String,
    pub versions: BTreeMap<String, VersionMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionMetadata {
    pub version: String,
    pub integrity: String,
    pub tarball: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

pub struct RegistryClient {
    client: Client,
    base_url: String,
    token: Option<String>,
}

impl RegistryClient {
    pub fn new(base_url: &str, token: Option<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
        }
    }

    fn auth_header(&self) -> Option<String> {
        self.token.as_ref().map(|t| format!("Bearer {}", t))
    }

    /// GET /packages/:name — fetch package metadata
    pub async fn get_package(&self, name: &str) -> Result<PackageMetadata> {
        let url = format!("{}/packages/{}", self.base_url, name);
        let mut req = self.client.get(&url);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(AgmError::Registry(format!(
                "failed to fetch package: HTTP {}",
                resp.status()
            )));
        }
        Ok(resp.json().await?)
    }

    /// GET /packages/:name/:version — fetch version metadata
    pub async fn get_version(&self, name: &str, version: &str) -> Result<VersionMetadata> {
        let url = format!("{}/packages/{}/{}", self.base_url, name, version);
        let mut req = self.client.get(&url);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(AgmError::Registry(format!(
                "failed to fetch version: HTTP {}",
                resp.status()
            )));
        }
        Ok(resp.json().await?)
    }

    /// GET /packages/:name/-/:tarball — download tarball
    pub async fn download_tarball(&self, name: &str, tarball: &str, dest: &Path) -> Result<()> {
        let url = format!("{}/packages/{}/-/{}", self.base_url, name, tarball);
        let mut req = self.client.get(&url);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(AgmError::Registry(format!(
                "failed to download tarball: HTTP {}",
                resp.status()
            )));
        }
        let bytes = resp.bytes().await?;
        std::fs::write(dest, bytes)?;
        Ok(())
    }

    /// PUT /packages/:name — publish
    pub async fn publish(&self, name: &str, tarball_path: &Path) -> Result<()> {
        let url = format!("{}/packages/{}", self.base_url, name);
        let data = std::fs::read(tarball_path)?;
        let mut req = self.client.put(&url).body(data);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(AgmError::Registry(format!(
                "failed to publish: HTTP {}",
                resp.status()
            )));
        }
        Ok(())
    }
}
