//! Web PTY 终端服务库入口。

use anyhow::Context;
use axum::Router;
use config::Config;
use session_state::SessionState;
use ws_auth::{WsAuth, is_loopback_host};

pub mod config;
pub mod http_routes;
pub mod pty_session;
pub mod session_state;
pub mod ws_auth;
pub mod ws_handler;

#[cfg(test)]
mod config_test;
#[cfg(test)]
mod http_routes_test;
#[cfg(test)]
mod pty_session_test;
#[cfg(test)]
mod ws_handler_test;

/// 启动 Web PTY 终端服务。
pub async fn start_server(config: Config) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("failed to bind TCP listener")?;
    let actual_port = listener.local_addr()?.port();

    // Host no-loopback ⇒ opt-in a exponerse en la red local, con token.
    // El token se imprime en consola (patrón Jupyter) y se exige en /ws.
    let token = if is_loopback_host(&config.host) {
        None
    } else {
        Some(uuid::Uuid::new_v4().simple().to_string())
    };

    let url = http_url(&config.host, actual_port);
    if let Some(t) = token.as_deref() {
        tracing::info!("Web PTY server (token): {}?token={}", url, t);
    } else {
        tracing::info!("Web PTY server: {}", url);
    }

    // 尝试自动打开浏览器
    open_browser(&url);

    let app = build_app(config, actual_port, token);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}

/// URL pública del server según el host de bind (localaddr para loopback).
fn http_url(host: &str, port: u16) -> String {
    let display = if is_loopback_host(host) {
        "127.0.0.1".to_string()
    } else {
        host.to_string()
    };
    format!("http://{}:{}", display, port)
}

/// Router completo del servicio. Se construye con el puerto **real** ya
/// resuelto y el token (si corresponde), para que `/ws` valide Origin/token
/// contra valores conocidos post-bind. Público para tests de integración.
pub fn build_app(config: Config, actual_port: u16, token: Option<String>) -> Router {
    let cwd = config.cwd.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string())
    });
    let state = SessionState::new(Some(cwd), config.initial_cmd.clone())
        .with_ws_auth(WsAuth::new(config.host, actual_port, token));

    Router::new()
        .route("/", axum::routing::get(http_routes::index))
        .route("/ws", axum::routing::get(ws_handler::ws_handler))
        .with_state(state)
}

/// 尝试用系统默认浏览器打开 URL。失败时静默跳过。
fn open_browser(url: &str) {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else if cfg!(target_os = "linux") {
        std::process::Command::new("xdg-open").arg(url).spawn()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn()
    } else {
        return;
    };

    match result {
        Ok(_) => tracing::info!("browser opened: {}", url),
        Err(e) => tracing::warn!("failed to open browser: {e}"),
    }
}

/// 优雅关闭信号监听。
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}
