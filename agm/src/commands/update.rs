use crate::error::Result;
use std::path::PathBuf;

pub fn execute(project_dir: Option<PathBuf>) -> Result<()> {
    let dir = project_dir.unwrap_or_else(|| PathBuf::from("."));
    println!(
        "Checking for updates in {}... (v1: manual update)",
        dir.display()
    );
    println!("Edit agm.json version ranges and run `agm install` to upgrade.");
    Ok(())
}
