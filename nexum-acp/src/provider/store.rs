use std::path::{Path, PathBuf};

use anyhow::Result;

use super::config::NexumConfig;

/// 配置文件路径：~/.nexum/settings.json
pub fn config_path() -> PathBuf {
    nexum_agent::config_home::nexum_home().join("settings.json")
}

/// 工作区配置文件路径：{cwd}/.nexum/settings.json
/// 文件不存在时返回 None
pub fn workspace_config_path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let path = nexum_agent::config_home::nexum_workspace_dir(&cwd).join("settings.json");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// 加载配置（全局 + 工作区合并），文件不存在时返回默认空配置
///
/// 先加载 ~/.nexum/settings.json 获取全局配置，
/// 再检测当��工作目录的 .nexum/settings.json 是否存在，
/// 若存在则加载并以工作区字段覆盖全局对应字段。
pub fn load() -> Result<NexumConfig> {
    let mut merged = load_from(&config_path())?;
    if let Some(ws_path) = workspace_config_path() {
        let workspace = load_from(&ws_path)?;
        merged.config.merge_overrides(workspace.config);
    }
    Ok(merged)
}

/// 从指定路径加载配置
pub fn load_from(path: &Path) -> Result<NexumConfig> {
    if !path.exists() {
        return Ok(NexumConfig::default());
    }
    let content = std::fs::read_to_string(path)?;
    let cfg: NexumConfig = serde_json::from_str(&content)?;
    Ok(cfg)
}

/// 原子写回配置文件（先写临时文件，再 rename，避免写入中断导致文件损坏）
pub fn save(cfg: &NexumConfig) -> Result<()> {
    save_to(cfg, &config_path())
}

/// 将配置写入指定路径
pub fn save_to(cfg: &NexumConfig, path: &Path) -> Result<()> {
    // 确保目录存在
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(cfg)?;

    // atomic write
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, content)?;
    std::fs::rename(&tmp_path, path)?;

    Ok(())
}

#[cfg(test)]
#[path = "store_test.rs"]
mod store_tests;
