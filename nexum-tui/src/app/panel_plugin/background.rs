// 后台任务编排：spawn install_counts / official marketplace 刷新
//
// 从原 panel_plugin.rs::open_plugin_panel 拆分而来（原 333-398 行）。
//
// [TRAP] bg_event_tx 发送的 AgentEvent::PluginActionCompleted 必须保持 plugin_id
// 和 action 字段完全一致：agent_events_plugin.rs:66/82 按 plugin_id + action 匹配
// （如 "__install_counts__"+"install_counts_refresh"、official_name+"refresh"）。
// 禁止重命名 action 字符串，否则面板刷新事件丢失。
//
// [TRAP] 4 处 tokio::spawn 捕获 bg_event_tx.clone()，需保持 clone 语义（tx 是
// mpsc Sender，clone 后各 spawn 独立所有权）。禁止传 &self 引用进 spawn 闭包。

use crate::app::AgentEvent;

/// 后台刷新安装量数据（若缓存过期）
///
/// 发送 plugin_id="__install_counts__" + action="install_counts_refresh"。
pub(crate) fn spawn_install_counts_refresh(tx: tokio::sync::mpsc::Sender<AgentEvent>) {
    tokio::spawn(async move {
        let result = nexum_middlewares::plugin::fetch_install_counts().await;
        if result.is_some() {
            let _ = tx
                .send(AgentEvent::PluginActionCompleted {
                    plugin_id: "__install_counts__".to_string(),
                    action: "install_counts_refresh".to_string(),
                    success: true,
                    message: String::new(),
                })
                .await;
        }
    });
}

/// 后台刷新 official marketplace（仅当 discover 列表为空时）
///
/// 发送 plugin_id=official_name + action="refresh"。
///
/// [TRAP] discover_was_empty 驱动后续 official marketplace 刷新（隐式状态机）。
/// 调用方必须显式传递该 bool，禁止丢弃判断。
pub(crate) fn spawn_official_marketplace_refresh(
    tx: tokio::sync::mpsc::Sender<AgentEvent>,
    official_name: String,
) {
    use nexum_middlewares::plugin::MarketplaceSource;
    let official_source = MarketplaceSource::GitHub {
        repo: "anthropics/claude-plugins-official".into(),
    };
    tokio::spawn(async move {
        use nexum_middlewares::plugin::marketplace::refresh_marketplace;
        match refresh_marketplace(&official_source, &official_name).await {
            Ok((_manifest, _install_location)) => {
                // 同步到 known_marketplaces 以记录 install_location
                if let Ok(mut marketplaces) =
                    nexum_middlewares::plugin::load_known_marketplaces(None)
                {
                    if let Some(km) = marketplaces
                        .iter_mut()
                        .find(|km| km.source == official_source)
                    {
                        km.install_location = _install_location;
                        km.last_updated = chrono::Utc::now().to_rfc3339();
                        // [TRAP] save 失败不 panic（当前用 let _ = 忽略），
                        // 否则后台任务崩溃。
                        let _ =
                            nexum_middlewares::plugin::save_known_marketplaces(&marketplaces, None);
                    }
                }
                let _ = tx
                    .send(AgentEvent::PluginActionCompleted {
                        plugin_id: official_name,
                        action: "refresh".to_string(),
                        success: true,
                        message: String::new(),
                    })
                    .await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "official marketplace \u{521d}\u{59cb}\u{5237}\u{65b0}\u{5931}\u{8d25}");
            }
        }
    });
}

/// 后台获取 marketplace 内容（add 路径）
///
/// 发送 plugin_id=name + action="add"。
pub(crate) fn spawn_marketplace_content_fetch(
    tx: tokio::sync::mpsc::Sender<AgentEvent>,
    source: nexum_middlewares::plugin::MarketplaceSource,
    name: String,
) {
    let name_clone = name.clone();
    tokio::spawn(async move {
        use nexum_middlewares::plugin::marketplace::refresh_marketplace;
        match refresh_marketplace(&source, &name_clone).await {
            Ok((_manifest, install_location)) => {
                // 更新 installLocation 和 lastUpdated
                if let Ok(mut mkt_places) = nexum_middlewares::plugin::load_known_marketplaces(None)
                {
                    if let Some(entry) = mkt_places.iter_mut().find(|km| km.source == source) {
                        entry.install_location = install_location;
                        entry.last_updated = chrono::Utc::now().to_rfc3339();
                        // [TRAP] save 失败不 panic（用 let _ = 忽略）
                        let _ =
                            nexum_middlewares::plugin::save_known_marketplaces(&mkt_places, None);
                    }
                }
                let _ = tx
                    .send(AgentEvent::PluginActionCompleted {
                        plugin_id: name_clone.clone(),
                        action: "add".to_string(),
                        success: true,
                        message: format!("Marketplace '{}' 内容已获取", name_clone),
                    })
                    .await;
            }
            Err(e) => {
                let _ = tx
                    .send(AgentEvent::PluginActionCompleted {
                        plugin_id: name_clone.clone(),
                        action: "add".to_string(),
                        success: false,
                        message: format!("获取内容失败: {}", e),
                    })
                    .await;
            }
        }
    });
}

/// 后台刷新指定 marketplace 缓存（update 路径）
///
/// 发送 plugin_id=name + action="refresh"。
pub(crate) fn spawn_marketplace_update_refresh(
    tx: tokio::sync::mpsc::Sender<AgentEvent>,
    source: nexum_middlewares::plugin::MarketplaceSource,
    name: String,
) {
    tokio::spawn(async move {
        use nexum_middlewares::plugin::marketplace::refresh_marketplace;
        match refresh_marketplace(&source, &name).await {
            Ok((_manifest, install_location)) => {
                if let Ok(mut marketplaces) =
                    nexum_middlewares::plugin::load_known_marketplaces(None)
                {
                    if let Some(entry) = marketplaces.iter_mut().find(|km| {
                        nexum_middlewares::plugin::MarketplaceManager::extract_name(&km.source)
                            == name
                    }) {
                        entry.install_location = install_location;
                        entry.last_updated = chrono::Utc::now().to_rfc3339();
                        let _ =
                            nexum_middlewares::plugin::save_known_marketplaces(&marketplaces, None);
                    }
                }
                let _ = tx
                    .send(AgentEvent::PluginActionCompleted {
                        plugin_id: name.clone(),
                        action: "refresh".to_string(),
                        success: true,
                        message: format!("Marketplace '{}' 已更新", name),
                    })
                    .await;
            }
            Err(e) => {
                let _ = tx
                    .send(AgentEvent::PluginActionCompleted {
                        plugin_id: name.clone(),
                        action: "refresh".to_string(),
                        success: false,
                        message: format!("更新失败: {}", e),
                    })
                    .await;
            }
        }
    });
}
