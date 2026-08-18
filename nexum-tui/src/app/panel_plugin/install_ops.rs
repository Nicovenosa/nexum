// Plugin 安装编排：plugin_install_by_marketplace
//
// 从原 panel_plugin.rs 拆分而来（原 633-708 行）。
// 前置校验 + 进度消息 + spawn 异步安装 + 发送 PluginActionCompleted 事件。

impl crate::app::App {
    /// 从 marketplace 缓存安装指定插件（供 /plugin install 斜杠命令使用）
    pub fn plugin_install_by_marketplace(
        &mut self,
        name: &str,
        marketplace: &str,
    ) -> anyhow::Result<()> {
        use nexum_middlewares::plugin::{
            install_plugin, marketplaces_cache_dir, InstallScope, MarketplaceManager,
        };

        // 1. 检查 marketplace 是否存在
        // [TRAP] unwrap_or_default() 吞错误：首次运行无配置文件时返回空列表是预期行为
        let known = nexum_middlewares::plugin::load_known_marketplaces(None).unwrap_or_default();
        let mp_exists = known
            .iter()
            .any(|km| MarketplaceManager::extract_name(&km.source) == marketplace);
        if !mp_exists {
            anyhow::bail!(
                "未找到 marketplace '{}'。请先通过 /plugin marketplace add 添加。",
                marketplace
            );
        }

        // 2. 检查是否已安装
        // [TRAP] unwrap_or_default() 吞错误：首次运行无 ~/.claude 时返回空列表是预期行为
        let installed = nexum_middlewares::plugin::load_installed_plugins(None).unwrap_or_default();
        let plugin_id = format!("{}@{}", name, marketplace);
        if installed.plugins.iter().any(|p| p.id == plugin_id) {
            self.session_mgr.current_mut().messages.view_messages.push(
                crate::app::MessageViewModel::system(format!(
                    "插件 '{}' 已安装，无需重复安装",
                    plugin_id
                )),
            );
            return Ok(());
        }

        // 3. 推送进度消息
        self.session_mgr.current_mut().messages.view_messages.push(
            crate::app::MessageViewModel::system(format!("正在安装 {}@{} ...", name, marketplace)),
        );

        // 4. Spawn 异步安装
        // [TRAP] bg_event_tx.clone() 保持 clone 语义，各 spawn 独立所有权
        let name = name.to_string();
        let mkt = marketplace.to_string();
        let cache_dir = marketplaces_cache_dir();
        let claude_dir = dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".claude");
        let tx = self.services.bg_event_tx.clone();

        tokio::spawn(async move {
            let result = install_plugin(
                &name,
                &mkt,
                InstallScope::User,
                &cache_dir,
                &claude_dir,
                None,
            )
            .await;
            let plugin_id = format!("{}@{}", name, mkt);
            let (success, msg) = match &result {
                Ok(r) => (true, format!("已安装: {} v{}", r.id, r.version)),
                Err(e) => (false, format!("安装失败: {}", e)),
            };
            // [TRAP] plugin_id + action="install" 字符串不可变，agent_events_plugin.rs
            // 按此匹配刷新面板
            let _ = tx
                .send(crate::app::AgentEvent::PluginActionCompleted {
                    plugin_id,
                    action: "install".to_string(),
                    success,
                    message: msg,
                })
                .await;
        });

        Ok(())
    }
}
