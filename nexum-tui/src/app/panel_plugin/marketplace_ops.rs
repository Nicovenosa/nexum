// Marketplace 持久化 CRUD：add / delete / update
//
// 从原 panel_plugin.rs 拆分而来（原 473-630、711-778 行）。
//
// 三个方法均为 impl App，承担配置文件读写 + UI 视图模型注入 + 面板刷新 +
// 后台 spawn 四件事。配置持久化与 UI 反馈的进一步分离留待后续 PR。
//
// [TRAP] session_mgr.current_mut().messages.view_messages.push 产生的是
// SystemNote 类型 VM，依赖 MessagePipeline 的 ephemeral_notes 锚点机制。
// 禁止改用 BaseMessage::system（会触发系统提示词 hoist 问题）。
//
// [TRAP] marketplace_delete_and_save 末尾的 cursor 钳位逻辑依赖 PluginPanel
// 内部状态。必须保持「先 open_plugin_panel 重建面板，再 get_mut 设置 view 和
// cursor」的两步顺序，不能合并（open_plugin_panel 会重置面板实例，覆盖 cursor）。

impl crate::app::App {
    /// 添加并保存 marketplace
    ///
    /// 这个方法是同步的，但会启动后台任务获取内容
    pub fn marketplace_add_and_save(&mut self, input: &str) -> anyhow::Result<()> {
        use nexum_middlewares::plugin::{
            parse_marketplace_input, save_known_marketplaces, KnownMarketplace, MarketplaceManager,
        };

        // 解析输入
        let source =
            parse_marketplace_input(input).map_err(|e| anyhow::anyhow!("解析失败: {}", e))?;

        // 加载现有的 marketplaces
        // [TRAP] unwrap_or_default() 吞错误：首次运行无配置文件时返回空列表是预期行为
        let mut marketplaces =
            nexum_middlewares::plugin::load_known_marketplaces(None).unwrap_or_default();

        // 检查是否已存在
        for existing in &marketplaces {
            if existing.source == source {
                anyhow::bail!("Marketplace 已存在");
            }
        }

        // 提取名称
        let name = MarketplaceManager::extract_name(&source);

        // 创建新条目（初始状态：install_location 和 last_updated 为空）
        let new_entry = KnownMarketplace {
            source: source.clone(),
            install_location: String::new(),
            auto_update: false,
            last_updated: String::new(),
        };

        marketplaces.push(new_entry);

        // 保存配置
        save_known_marketplaces(&marketplaces, None)?;

        // 显示成功消息
        self.session_mgr.current_mut().messages.view_messages.push(
            crate::app::MessageViewModel::system(format!(
                "Marketplace 已添加: {} (正在获取内容...)",
                name
            )),
        );

        // 刷新面板以显示新添加的 marketplace
        self.open_plugin_panel();

        // 启动后台任务获取内容并更新 installLocation
        let tx = self.services.bg_event_tx.clone();
        super::background::spawn_marketplace_content_fetch(tx, source, name);

        Ok(())
    }

    /// 删除并保存 marketplace
    pub fn marketplace_delete_and_save(&mut self, name: &str) -> anyhow::Result<()> {
        // 加载现有的 marketplaces
        // [TRAP] unwrap_or_default() 吞错误：首次运行无配置文件时返回空列表是预期行为
        let marketplaces =
            nexum_middlewares::plugin::load_known_marketplaces(None).unwrap_or_default();

        // 过滤掉要删除的 marketplace（通过名称匹配）
        let filtered: Vec<_> = marketplaces
            .into_iter()
            .filter(|km| {
                let km_name =
                    super::source_helpers::extract_marketplace_name_for_delete(&km.source);
                km_name != name
            })
            .collect();

        // 保存
        nexum_middlewares::plugin::save_known_marketplaces(&filtered, None)?;

        // 显示成功消息
        self.session_mgr.current_mut().messages.view_messages.push(
            crate::app::MessageViewModel::system(format!("Marketplace 已移除: {}", name)),
        );

        // [TRAP] 必须保持「先 open_plugin_panel 重建面板，再 get_mut 设置 view 和
        // cursor」的两步顺序。open_plugin_panel 会重置面板实例，覆盖 cursor。
        // 刷新面板并恢复到 Marketplaces 视图
        self.open_plugin_panel();
        if let Some(ref mut p) = self
            .global_panels
            .get_mut::<crate::app::plugin_panel::PluginPanel>()
        {
            p.view = crate::app::plugin_panel::PluginPanelView::Marketplaces;
            // 确保 cursor 不越界
            let max = p.marketplace_entries.len();
            if p.marketplace_list.cursor() > max {
                p.marketplace_list.move_cursor_to(max);
            }
        }

        Ok(())
    }

    /// 刷新指定 marketplace 缓存（供 /plugin marketplace update 斜杠命令使用）
    pub fn marketplace_update_and_refresh(&mut self, name: &str) -> anyhow::Result<()> {
        use nexum_middlewares::plugin::MarketplaceManager;

        // 1. 查找 marketplace
        // [TRAP] unwrap_or_default() 吞错误：首次运行无配置文件时返回空列表是预期行为
        let known = nexum_middlewares::plugin::load_known_marketplaces(None).unwrap_or_default();
        let target = known
            .iter()
            .find(|km| MarketplaceManager::extract_name(&km.source) == name);

        let source = match target {
            Some(km) => km.source.clone(),
            None => {
                anyhow::bail!(
                    "未找到 marketplace '{}'。请先通过 /plugin marketplace add 添加。",
                    name
                );
            }
        };

        // 2. 推送进度消息
        self.session_mgr.current_mut().messages.view_messages.push(
            crate::app::MessageViewModel::system(format!("正在刷新 marketplace '{}' ...", name)),
        );

        // 3. Spawn 后台刷新
        let name = name.to_string();
        let tx = self.services.bg_event_tx.clone();
        super::background::spawn_marketplace_update_refresh(tx, source, name);

        Ok(())
    }
}
