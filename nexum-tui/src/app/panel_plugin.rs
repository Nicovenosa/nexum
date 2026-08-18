// panel_plugin.rs — Facade 入口
//
// 原本是 779 行的 god file，按业务域拆分为 panel_plugin/ 子目录下的多个文件。
// 本文件仅声明子模块；所有 App 方法通过各子模块的 `impl crate::app::App` 块
// 定义，对下游调用方（command/panel/*.rs、agent_events_plugin.rs）零改动。
//
// 设计模式：Module-per-Feature + Facade（见 .tmp/god-file-analysis.md）。
//
// 子模块职责：
// - entries.rs          面板入口方法（open_cron/mcp/tasks/plugin + close_plugin）
// - plugin_loader.rs     Plugin 面板纯数据加载层（load_plugin_panel_data）
// - source_helpers.rs    MarketplaceSource 标签/名称提取纯函数
// - marketplace_ops.rs   Marketplace 持久化 CRUD（add/delete/update）
// - install_ops.rs       Plugin 安装编排（plugin_install_by_marketplace）
// - background.rs        后台 spawn 任务编排抽象

mod background;
mod entries;
mod install_ops;
mod marketplace_ops;
mod plugin_loader;
mod source_helpers;
