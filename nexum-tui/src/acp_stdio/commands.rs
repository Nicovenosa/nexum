//! AvailableCommands 通知辅助，供 session/new 和 session/load 复用。

use agent_client_protocol::{
    schema::{AvailableCommandsUpdate, SessionId, SessionNotification, SessionUpdate},
    Client, ConnectionTo,
};

/// 扫描 skill 目录并发送 AvailableCommandsUpdate 通知。
pub(super) fn send_available_commands(
    cwd: &str,
    plugin_skill_roots: &[nexum_middlewares::skills::SkillRoot],
    session_id: &SessionId,
    cx: &ConnectionTo<Client>,
) {
    let disable_bundled = nexum_middlewares::skills::load_disable_bundled_skills();
    let skill_roots = nexum_middlewares::SkillsMiddleware::resolve_roots_static(
        cwd,
        plugin_skill_roots.to_vec(),
        disable_bundled, // Stdio 侧仅用于显示
    );
    let skills = nexum_middlewares::skills::scan_skill_roots(&skill_roots);
    let cmds = nexum_acp::dispatch::build_available_commands(&skills);
    let notif = SessionNotification::new(
        session_id.clone(),
        SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(cmds)),
    );
    let _ = cx.send_notification(notif);
}
