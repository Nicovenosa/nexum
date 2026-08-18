use super::*;
use crate::app::App;
use crate::command::Command;

async fn make_headless() -> App {
    let (app, _handle) = App::new_headless(80, 24).await;
    app
}

/// 辅助：获取最近一条系统消息文本
fn last_system_note(app: &App) -> Option<String> {
    app.session_mgr
        .current()
        .messages
        .view_messages
        .iter()
        .rev()
        .find(|vm| matches!(vm, crate::ui::message_view::MessageViewModel::SystemNote { .. }))
        .map(|vm| match vm {
            crate::ui::message_view::MessageViewModel::SystemNote { content, .. } => content.clone(),
            _ => unreachable!(),
        })
}

#[tokio::test]
async fn test_plugin_empty_args_opens_panel() {
    let mut app = make_headless().await;
    let cmd = PluginCommand;
    cmd.execute(&mut app, "");
    // 空参数应打开 Plugin Panel
    assert!(
        app.global_panels.get::<crate::app::plugin_panel::PluginPanel>().is_some(),
        "无参数应打开插件面板"
    );
}

#[tokio::test]
async fn test_plugin_marketplace_add_to_existing_shows_error() {
    let mut app = make_headless().await;
    let cmd = PluginCommand;
    // anthropics/claude-plugins-official 已内置，add 会触发"已存在"错误
    cmd.execute(&mut app, "marketplace add anthropics/claude-plugins-official");
    let msg = last_system_note(&app);
    assert!(msg.is_some(), "marketplace add（重复）应产生错误消息");
}

#[tokio::test]
async fn test_plugin_marketplace_update_missing_shows_error() {
    let mut app = make_headless().await;
    let cmd = PluginCommand;
    cmd.execute(&mut app, "marketplace update nonexistent-marketplace");
    let msg = last_system_note(&app);
    assert!(msg.is_some(), "marketplace update（不存在）应产生错误消息");
    assert!(msg.unwrap().contains("未找到"), "错误消息应提及未找到");
}

#[tokio::test]
async fn test_plugin_install_missing_shows_error() {
    let mut app = make_headless().await;
    let cmd = PluginCommand;
    cmd.execute(&mut app, "install none@none");
    let msg = last_system_note(&app);
    assert!(msg.is_some(), "install（不存在）应产生错误消息");
}

#[tokio::test]
async fn test_plugin_unknown_subcommand_shows_usage() {
    let mut app = make_headless().await;
    let cmd = PluginCommand;
    cmd.execute(&mut app, "unknown sub command");
    let msg = last_system_note(&app);
    assert!(msg.is_some(), "未知子命令应显示用法提示");
    // 默认 locale 可能是 en/zh-CN，两种文案都接受
    let text = msg.unwrap();
    assert!(
        text.contains("用法") || text.contains("Usage"),
        "未知子命令的消息应包含用法说明，实际: {}",
        text
    );
}
