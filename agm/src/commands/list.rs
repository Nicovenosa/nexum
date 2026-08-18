use crate::error::{AgmError, Result};
use crate::resolver::is_git_dep;
use crate::types::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn execute(project_dir: Option<PathBuf>) -> Result<()> {
    let dir = project_dir.unwrap_or_else(|| PathBuf::from("."));
    let manifest_path = dir.join("agm.json");

    if !manifest_path.exists() {
        return Err(AgmError::ManifestNotFound);
    }

    let manifest = ProjectManifest::load(&manifest_path)?;
    let lock_path = dir.join("agm.lock.json");
    let lock = if lock_path.exists() {
        Some(LockFile::load(&lock_path)?)
    } else {
        None
    };

    println!("Dependencies for {}:\n", manifest.name);

    print_section("skills", &manifest.skills, &lock);
    print_section("agents", &manifest.agents, &lock);
    print_section("mcp", &manifest.mcp, &lock);

    if !manifest.overrides.is_empty() {
        println!("Overrides:");
        for (name, version) in &manifest.overrides {
            println!("  {} → {}", name, version);
        }
    }

    Ok(())
}

fn print_section(label: &str, deps: &BTreeMap<String, DependencySpec>, lock: &Option<LockFile>) {
    if deps.is_empty() {
        return;
    }
    println!("[{}]", label);
    for (name, spec) in deps {
        let source = if is_git_dep(name) { "git" } else { "registry" };
        let installed = lock.as_ref().and_then(|l| {
            l.packages
                .iter()
                .find(|(k, _)| k.starts_with(name))
                .map(|(_, p)| p.targets.join(", "))
        });

        let filters = match spec {
            DependencySpec::Simple(_) => String::new(),
            DependencySpec::Detailed { pick, omit, .. } => {
                let mut parts = Vec::new();
                if !pick.is_empty() {
                    parts.push(format!("pick=[{}]", pick.join(", ")));
                }
                if !omit.is_empty() {
                    parts.push(format!("omit=[{}]", omit.join(", ")));
                }
                if parts.is_empty() {
                    String::new()
                } else {
                    format!(" {}", parts.join(" "))
                }
            }
        };

        match installed {
            Some(targets) if !targets.is_empty() => {
                println!(
                    "  ✓ {} {} ({}) [installed: {}]{}",
                    name,
                    spec.version(),
                    source,
                    targets,
                    filters
                );
            }
            _ => {
                println!(
                    "  ✗ {} {} ({}) [pending]{}",
                    name,
                    spec.version(),
                    source,
                    filters
                );
            }
        }
    }
}
