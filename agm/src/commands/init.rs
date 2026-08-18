use crate::error::Result;
use crate::types::ProjectManifest;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn execute(project_dir: Option<PathBuf>) -> Result<()> {
    let dir = project_dir.unwrap_or_else(|| PathBuf::from("."));
    let manifest_path = dir.join("agm.json");

    if manifest_path.exists() {
        println!("agm.json already exists in {}", dir.display());
        return Ok(());
    }

    let name = dir
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "my-agent-project".into());

    let manifest = ProjectManifest {
        name,
        version: "1.0.0".into(),
        description: String::new(),
        author: String::new(),
        registry: None,
        targets: vec!["claude".into()],
        skills: BTreeMap::new(),
        agents: BTreeMap::new(),
        mcp: BTreeMap::new(),
        overrides: BTreeMap::new(),
    };

    manifest.save(&manifest_path)?;
    println!("Created {}", manifest_path.display());
    Ok(())
}
