// Plugin 面板数据加载层（open_plugin_panel 的纯数据加载部分）
//
// 从原 panel_plugin.rs::open_plugin_panel 拆分而来（原 55-330 行）。
//
// 设计目标（见 .tmp/god-file-analysis.md）：
// - 纯数据加载 + 转换，无 &mut self、无 UI 副作用、无 spawn
// - 返回中间数据结构 PluginPanelLoadResult，由 entry.rs 负责 spawn / open_panel
//
// [TRAP] 多处 unwrap_or_default() / unwrap_or_else() 吞错误（67/70/71/184/484/
// 572/644/656/717）。保持原降级策略——load_installed_plugins 失败返回空列表是
// 预期行为（首次运行无 ~/.claude）。禁止改为 ? 传播，否则首次运行打开 Plugin
// 面板会报错退出。
//
// [TRAP] 所有 use nexum_middlewares::plugin::{...} 语句在函数内部（而非文件顶部），
// 这是项目惯例（避免 mod.rs 顶部 import 爆炸）。保持方法内 use 风格。

use crate::app::plugin_panel::{
    DiscoverPlugin, MarketplaceViewEntry, MarketplaceViewStatus, PluginEntry, PluginItemType,
};

/// open_plugin_panel 数据加载阶段的输出
///
/// discover_was_empty 驱动后续 official marketplace 后台刷新（隐式状态机）。
pub(crate) struct PluginPanelLoadResult {
    pub panel: crate::app::plugin_panel::PluginPanel,
    pub discover_was_empty: bool,
}

/// 加载并组装 PluginPanel 数据
///
/// 13 个步骤：读盘加载已安装插件 → 加载 settings → 解析 manifest → 提取
/// commands/skills/agents/mcp_servers → 按 scope 排序 → 加载 marketplace 缓存 →
/// 合并 extraKnownMarketplaces → 注入 official 占位符 → 构建 discover_plugins →
/// 加载安装量数据 → 排序。
pub(crate) fn load_plugin_panel_data() -> PluginPanelLoadResult {
    use nexum_middlewares::plugin::{
        load_claude_settings, load_installed_plugins, load_known_marketplaces,
        marketplaces_cache_dir, MarketplaceManager,
    };

    let claude_dir = dirs_next::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".claude");
    // claude_dir 在本函数内不再使用（早期重构遗留），仅保留计算以避免删除
    // 影响调用方预期。后续 plugin_install_by_marketplace 会重新计算。
    let _ = claude_dir;

    let installed = load_installed_plugins(None).unwrap_or_default();
    let settings = load_claude_settings(None).unwrap_or_default();
    let enabled_ids: std::collections::HashSet<&str> = settings
        .enabled_plugins
        .iter()
        .map(|s| s.as_str())
        .collect();

    // 已安装插件 ID 集合（用于 Discover 标记 installed）
    let installed_ids: std::collections::HashSet<String> =
        installed.plugins.iter().map(|p| p.id.clone()).collect();

    let entries = build_plugin_entries(&installed, &enabled_ids);

    // --- 加载 Discover 数据 ---
    let cache_base = marketplaces_cache_dir();
    // 确保缓存目录存在（首次运行时 ~/.claude/ 可能不存在）
    let _ = std::fs::create_dir_all(&cache_base);
    let _ = cache_base;
    let mgr = MarketplaceManager::new(None);
    let known = load_known_marketplaces(None).unwrap_or_default();

    // 构建 discover_plugins：从已缓存的 marketplace manifest 中提取
    let mut discover_plugins: Vec<DiscoverPlugin> = Vec::new();
    let mut marketplace_view_entries: Vec<MarketplaceViewEntry> = Vec::new();

    // 合并 extraKnownMarketplaces
    let mut all_known = known;
    for extra in &settings.extra_known_marketplaces {
        let extra_json = serde_json::to_string(&extra.source).unwrap_or_default();
        let already_exists = all_known
            .iter()
            .any(|km| serde_json::to_string(&km.source).unwrap_or_default() == extra_json);
        if !already_exists {
            // 将 DeclaredMarketplace 转换为 KnownMarketplace
            all_known.push(nexum_middlewares::plugin::KnownMarketplace::from(
                extra.clone(),
            ));
        }
    }

    // 确保 official marketplace 已注册
    use nexum_middlewares::plugin::MarketplaceSource;
    let has_official = all_known.iter().any(|km| match &km.source {
        MarketplaceSource::GitHub { repo } => repo == "anthropics/claude-plugins-official",
        _ => false,
    });
    if !has_official {
        all_known.push(nexum_middlewares::plugin::KnownMarketplace {
            source: MarketplaceSource::GitHub {
                repo: "anthropics/claude-plugins-official".into(),
            },
            install_location: String::new(), // 占位符，实际安装时会更新
            auto_update: true,
            last_updated: String::new(), // 占位符，实际安装时会更新
        });
    }

    for km in &all_known {
        let name = MarketplaceManager::extract_name(&km.source);

        // 优先从 install_location 加载，如果不存在则使用默认路径
        // 注意：Url 类型的 install_location 指向 .json 文件，其他类型指向目录
        let cached_manifest = if !km.install_location.is_empty() {
            use nexum_middlewares::plugin::marketplace::{
                find_marketplace_json, read_manifest_from_path,
            };
            let cache_path = std::path::Path::new(&km.install_location);

            // 判断是文件还是目录
            if cache_path.is_file() {
                // 直接是 .json 文件（Url 类型）
                read_manifest_from_path(cache_path).ok()
            } else {
                // 是目录，需要查找 marketplace.json
                find_marketplace_json(cache_path).and_then(|p| read_manifest_from_path(&p).ok())
            }
        } else {
            mgr.try_load_cache(&km.source, &name)
        };

        let (status, plugin_count) = if let Some(ref manifest) = cached_manifest {
            let count = manifest.plugins.len();
            (MarketplaceViewStatus::Cached, count)
        } else {
            (MarketplaceViewStatus::Stale, 0)
        };

        // 构建 discover 列表
        if let Some(ref manifest) = cached_manifest {
            for p in &manifest.plugins {
                let plugin_id = format!("{}@{}", p.name, name);
                let is_installed = installed_ids.contains(&plugin_id);
                discover_plugins.push(DiscoverPlugin {
                    name: p.name.clone(),
                    description: p.description.clone(),
                    marketplace: name.clone(),
                    version: p.version.clone(),
                    author: p.author.as_ref().map(|a| a.name.clone()),
                    installed: is_installed,
                    plugin_id,
                    install_count: None,
                });
            }
        }

        // source label（从 source_helpers 复用）
        let source_label = super::source_helpers::format_source_label(&km.source);

        // 统计该 marketplace 的已安装插件数
        let installed_count = installed_ids
            .iter()
            .filter(|id| id.ends_with(&format!("@{}", name)))
            .count();

        marketplace_view_entries.push(MarketplaceViewEntry {
            name: name.clone(),
            source: km.source.clone(),
            source_label,
            plugin_count,
            installed_count,
            status,
            last_updated: if km.last_updated.is_empty() {
                None
            } else {
                Some(km.last_updated.clone())
            },
            auto_update: km.auto_update,
        });
    }

    // 注入安装量数据并排序
    let install_counts = nexum_middlewares::plugin::load_install_counts();
    if let Some(ref counts) = install_counts {
        for dp in &mut discover_plugins {
            // 远程数据 key 格式为 "plugin-name@marketplace-name"，与 plugin_id 一致
            dp.install_count = counts.get(&dp.plugin_id).copied();
        }
        // 安装量降序 -> 同安装量按字母序
        discover_plugins.sort_by(|a, b| {
            let ca = a.install_count.unwrap_or(0);
            let cb = b.install_count.unwrap_or(0);
            cb.cmp(&ca).then_with(|| a.name.cmp(&b.name))
        });
    } else {
        // 无安装量数据，按字母排序
        discover_plugins.sort_by(|a, b| a.name.cmp(&b.name));
    }

    let discover_was_empty = discover_plugins.is_empty();

    let mut panel = crate::app::plugin_panel::PluginPanel::new(entries);
    panel.discover_plugins = discover_plugins;
    panel.marketplace_entries = marketplace_view_entries;
    panel.sync_marketplace_list_items();

    PluginPanelLoadResult {
        panel,
        discover_was_empty,
    }
}

/// 解析已安装插件列表为 PluginEntry（含 manifest 加载与降级）
fn build_plugin_entries(
    installed: &nexum_middlewares::plugin::InstalledPlugins,
    enabled_ids: &std::collections::HashSet<&str>,
) -> Vec<PluginEntry> {
    use nexum_middlewares::plugin::load_plugin_manifest;

    let mut entries: Vec<PluginEntry> = Vec::new();
    for p in &installed.plugins {
        let enabled = enabled_ids.contains(p.id.as_str());

        let manifest_result = load_plugin_manifest(&p.install_path);
        let (plugin_type, load_error, description, author, commands, skills, agents, mcp_servers) =
            match &manifest_result {
                Ok(m) => {
                    // 统一显示为 Plugin 类型
                    let ptype = PluginItemType::Plugin;
                    let desc = m.description.clone();
                    let auth = m.author.as_ref().map(|a| a.name.clone());
                    let cmds = m
                        .commands
                        .as_ref()
                        .map(|c| {
                            c.iter()
                                .filter_map(|cmd| match cmd {
                                    nexum_middlewares::plugin::PluginCommandEntry::Full(fc) => {
                                        fc.name.clone().or_else(|| {
                                            std::path::Path::new(&fc.path)
                                                .file_stem()
                                                .and_then(|s| s.to_str().map(String::from))
                                        })
                                    }
                                    nexum_middlewares::plugin::PluginCommandEntry::Path(p) => {
                                        std::path::Path::new(p)
                                            .file_stem()
                                            .and_then(|s| s.to_str().map(String::from))
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let sks = m.skills.clone().unwrap_or_default();
                    let ags = m
                        .agents
                        .as_ref()
                        .map(|a| a.iter().map(|ag| ag.name.clone()).collect())
                        .unwrap_or_default();
                    let mcps = m
                        .mcp_servers
                        .as_ref()
                        .map(|s| s.keys().cloned().collect())
                        .unwrap_or_default();
                    (ptype, None, desc, auth, cmds, sks, ags, mcps)
                }
                Err(e) => (
                    PluginItemType::Plugin,
                    Some(e.to_string()),
                    String::new(),
                    None,
                    vec![],
                    vec![],
                    vec![],
                    vec![],
                ),
            };

        entries.push(PluginEntry {
            id: p.id.clone(),
            name: p.name.clone(),
            plugin_type,
            marketplace: p.marketplace.clone(),
            enabled,
            scope: p.scope,
            version: p.version.clone(),
            install_path: p.install_path.clone(),
            project_path: p.project_path.clone(),
            load_error,
            description,
            author,
            commands,
            skills,
            agents,
            mcp_servers,
        });
    }

    // 按 scope 排序: Project 在前, User 在后
    entries.sort_by(|a, b| {
        let scope_order = |s: &nexum_middlewares::plugin::InstallScope| match s {
            nexum_middlewares::plugin::InstallScope::Project => 0,
            nexum_middlewares::plugin::InstallScope::Local => 1,
            nexum_middlewares::plugin::InstallScope::User => 2,
        };
        scope_order(&a.scope).cmp(&scope_order(&b.scope))
    });

    entries
}
