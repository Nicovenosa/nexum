use crate::config::AgmConfig;
use crate::error::{AgmError, Result};
use crate::installer::InstallContext;
use crate::types::ProjectManifest;
use std::path::PathBuf;

pub fn execute(target: &str, git_url: Option<&str>, project_dir: Option<PathBuf>) -> Result<()> {
    let dir = project_dir.unwrap_or_else(|| PathBuf::from("."));
    let manifest_path = dir.join("agm.json");

    // --git mode: install URL directly, auto-create agm.json if missing
    if let Some(url) = git_url {
        let manifest = if manifest_path.exists() {
            ProjectManifest::load(&manifest_path)?
        } else {
            let name = dir
                .canonicalize()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_else(|| "my-agent-project".into());
            ProjectManifest {
                name,
                version: "1.0.0".into(),
                description: String::new(),
                author: String::new(),
                registry: None,
                targets: vec![target.to_string()],
                skills: std::collections::BTreeMap::new(),
                agents: std::collections::BTreeMap::new(),
                mcp: std::collections::BTreeMap::new(),
                overrides: std::collections::BTreeMap::new(),
            }
        };

        let config = AgmConfig::load()?;
        println!("Installing from {} to {}...", url, target);
        let mut ctx = InstallContext::new(config, manifest, target, dir)?;
        ctx.install_from_git(url)?;
        println!("Done.");
        return Ok(());
    }

    // Normal mode: install from agm.json
    if !manifest_path.exists() {
        return Err(AgmError::ManifestNotFound);
    }

    let manifest = ProjectManifest::load(&manifest_path)?;
    let config = AgmConfig::load()?;

    println!("Installing to {}...", target);
    let mut ctx = InstallContext::new(config, manifest, target, dir)?;
    ctx.install_all()?;

    println!(
        "Done. Installed to .{}/skills/, .{}/agents/, .{}/mcp/",
        target, target, target
    );
    Ok(())
}
