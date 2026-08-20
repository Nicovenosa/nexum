//! 端到端集成测试：起真实 axum server，用 tokio-tungstenite client 连接验证协议。
//!
//! Usa el mismo `build_app` público que `start_server` (que embebe la validación
//! de Origin/token de /ws), y el client manda el header `Origin` que mandaría
//! el browser cuando la página la sirve este mismo proceso.

use std::time::Duration;

use axum::http::Request as HttpRequest;
use axum::Router;
use futures::StreamExt;
use tokio_tungstenite::tungstenite::Message;

use nexum_web_pty::config::Config;

fn build_app(port: u16) -> Router {
    let cfg = Config {
        host: "127.0.0.1".to_string(),
        port: 0,
        shell: None,
        cwd: None,
        initial_cmd: None,
        default_cols: 80,
        default_rows: 24,
    };
    nexum_web_pty::build_app(cfg, port, None)
}

async fn spawn_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = build_app(port);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    port
}

/// Request de upgrade WS con el Origin que mandaría una página del propio server.
fn upgrade_request(port: u16, query: &str) -> HttpRequest<()> {
    HttpRequest::builder()
        .uri(format!("ws://127.0.0.1:{port}/ws{query}"))
        .header("host", format!("127.0.0.1:{port}"))
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header(
            "sec-websocket-key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .header("origin", format!("http://127.0.0.1:{port}"))
        .body(())
        .unwrap()
}

/// 跨平台获取测试 shell + 退出命令。
fn exit_shell() -> (&'static str, Vec<&'static str>) {
    if cfg!(target_os = "windows") {
        (
            "powershell.exe",
            vec![
                "-NoProfile",
                "-NoLogo",
                "-NonInteractive",
                "-Command",
                "exit",
            ],
        )
    } else {
        ("bash", vec!["-c", "exit"])
    }
}

#[tokio::test]
async fn test_ws_connection_receives_exit_message_on_child_exit() {
    let port = spawn_server().await;
    let (shell, args) = exit_shell();
    let query = format!("?shell={shell}&args={}", args.join("+"));

    let (mut ws, _) = tokio_tungstenite::connect_async(upgrade_request(port, &query))
        .await
        .unwrap();

    // 收消息直到看到 [process exited ...]
    let mut saw_exit = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_secs(3), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) if t.contains("[process exited") => {
                saw_exit = true;
                break;
            }
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
    assert!(saw_exit, "应收到 [process exited ...]");
}

#[tokio::test]
async fn test_ws_connection_spawn_failure_sends_error_and_closes() {
    let port = spawn_server().await;
    let query = "?shell=/nonexistent/pty-test-shell";

    let (mut ws, _) = tokio_tungstenite::connect_async(upgrade_request(port, query))
        .await
        .unwrap();

    let mut saw_error = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_secs(3), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) if t.contains("[failed to spawn") => {
                saw_error = true;
                break;
            }
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
    assert!(saw_error, "应收到 [failed to spawn ...]");
}