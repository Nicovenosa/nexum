// MarketplaceSource 标签格式化与名称提取
//
// 从原 panel_plugin.rs 拆分而来：
// - format_source_label()（原 271-278 行，原为 open_plugin_panel 内联 match）
// - extract_marketplace_name_for_delete()（原 578-605 行，原为
//   marketplace_delete_and_save 内联 match）
//
// 消除两处重复解构，便于复用与单测。

use nexum_middlewares::plugin::MarketplaceSource;

/// 格式化 MarketplaceSource 为 label 字符串
///
/// 6 种变体（GitHub/Git/Url/File/Directory/Npm）输出不同 label。
pub(crate) fn format_source_label(source: &MarketplaceSource) -> String {
    match source {
        MarketplaceSource::GitHub { repo } => format!("github:{}", repo),
        MarketplaceSource::Git { url } => format!("git:{}", url),
        MarketplaceSource::Url { url } => format!("url:{}", url),
        MarketplaceSource::File { path } => format!("file:{}", path),
        MarketplaceSource::Directory { path } => format!("dir:{}", path),
        MarketplaceSource::Npm { package } => format!("npm:{}", package),
    }
}

/// 提取 marketplace 名称（用于删除时通过名称匹配）
///
/// 与 MarketplaceManager::extract_name 语义一致，但在删除路径中
/// 不依赖 MarketplaceManager（保持原逻辑：直接从 source 反推 name）。
pub(crate) fn extract_marketplace_name_for_delete(source: &MarketplaceSource) -> String {
    match source {
        MarketplaceSource::GitHub { repo } => {
            repo.split('/').next_back().unwrap_or(repo).to_string()
        }
        MarketplaceSource::Git { url } => url
            .split('/')
            .next_back()
            .and_then(|s| s.strip_suffix(".git"))
            .unwrap_or("marketplace")
            .to_string(),
        MarketplaceSource::Url { url } => {
            let last = url.split('/').next_back().unwrap_or("marketplace");
            last.strip_suffix(".json").unwrap_or(last).to_string()
        }
        MarketplaceSource::File { path } => std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("marketplace")
            .to_string(),
        MarketplaceSource::Directory { path } => std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("marketplace")
            .to_string(),
        MarketplaceSource::Npm { package } => {
            package.split('@').next().unwrap_or(package).to_string()
        }
    }
}
