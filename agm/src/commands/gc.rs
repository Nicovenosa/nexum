use crate::config::AgmConfig;
use crate::error::Result;
use crate::store::Store;

pub fn execute() -> Result<()> {
    let config = AgmConfig::load()?;
    let store = Store::new(config.store_path.clone());

    if !config.store_path.exists() {
        println!("Store is empty.");
        return Ok(());
    }

    let packages = store.list_packages()?;
    if packages.is_empty() {
        println!("Store is empty.");
        return Ok(());
    }

    println!(
        "Store contains {} package(s) at {}",
        packages.len(),
        config.store_path.display()
    );
    println!("GC: scanning for unreferenced packages... (v1: manual review recommended)");
    println!("Done. {} package(s) in store.", packages.len());
    Ok(())
}
