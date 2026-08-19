#[cfg(unix)]
#[test]
fn test_cron_runtime_is_owned_only_by_the_host() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let status = std::process::Command::new("bash")
        .arg(root.join("scripts/check-cron-runtime-ownership.sh"))
        .status()
        .expect("architecture guard must run");
    assert!(status.success(), "cron runtime ownership guard failed");
}
