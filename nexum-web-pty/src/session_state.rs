use std::sync::{Arc, Mutex};

use crate::ws_auth::WsAuth;

/// 服务端共享状态，用于 cwd 和 first-session 命令注入。
pub struct SessionState {
    /// 所有 shell 的工作目录。
    pub cwd: Option<String>,
    /// 第一个 shell 启动时自动注入的命令。
    pub initial_cmd: Option<String>,
    /// 是否已注入。
    first_session_done: Arc<Mutex<bool>>,
    /// Validación de Origin/token del endpoint /ws.
    pub ws_auth: WsAuth,
}

impl Clone for SessionState {
    fn clone(&self) -> Self {
        Self {
            cwd: self.cwd.clone(),
            initial_cmd: self.initial_cmd.clone(),
            first_session_done: Arc::clone(&self.first_session_done),
            ws_auth: self.ws_auth.clone(),
        }
    }
}

impl SessionState {
    pub fn new(cwd: Option<String>, initial_cmd: Option<String>) -> Self {
        Self {
            cwd,
            initial_cmd,
            first_session_done: Arc::new(Mutex::new(false)),
            ws_auth: WsAuth::new("127.0.0.1".to_string(), 0, None),
        }
    }

    /// Setea la política de auth del /ws (port real + token). Se llama en build_app.
    pub fn with_ws_auth(mut self, auth: WsAuth) -> Self {
        self.ws_auth = auth;
        self
    }

    /// 原子地尝试标记为已注入。返回 `true` 表示本调用者应执行注入。
    pub fn try_mark_done(&self) -> bool {
        let mut done = self.first_session_done.lock().unwrap();
        if *done {
            return false;
        }
        *done = true;
        true
    }
}
