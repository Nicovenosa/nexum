use crate::error::{AgmError, Result};
use crate::types::PackageManifest;
use std::path::PathBuf;

pub fn execute(registry_url: Option<String>, project_dir: Option<PathBuf>) -> Result<()> {
    let dir = project_dir.unwrap_or_else(|| PathBuf::from("."));
    let manifest_path = dir.join("agm.package.json");
    if !manifest_path.exists() {
        return Err(AgmError::Other(
            "agm.package.json not found. Are you in a package directory?".into(),
        ));
    }

    let pkg = PackageManifest::load(&manifest_path)?;
    println!("Publishing {}@{}...", pkg.name, pkg.version);
    println!("  skills:  {:?}", pkg.skills);
    println!("  agents:  {:?}", pkg.agents);
    println!("  mcp:     {:?}", pkg.mcp);
    let _ = registry_url;
    println!("Done (dry run — registry upload not implemented in v1)");
    Ok(())
}
