use super::ws_handler::WsQuery;
use axum::http::HeaderMap;
use crate::ws_auth::WsAuth;

fn query(shell: Option<String>, args: Option<String>, cols: Option<String>, rows: Option<String>) -> WsQuery {
    WsQuery {
        shell,
        args,
        cols,
        rows,
        token: None,
    }
}

fn auth(host: &str, port: u16, token: Option<&str>) -> WsAuth {
    WsAuth::new(
        host.to_string(),
        port,
        token.map(|t| t.to_string()),
    )
}

#[test]
fn test_ws_query_parses_shell_and_dimensions() {
    let q = query(
        Some("/bin/zsh".to_string()),
        Some("-l".to_string()),
        Some("100".to_string()),
        Some("30".to_string()),
    );
    let parsed = q.to_spawn_params();
    assert_eq!(parsed.shell, "/bin/zsh");
    assert_eq!(parsed.args, vec!["-l"]);
    assert_eq!(parsed.cols, 100);
    assert_eq!(parsed.rows, 30);
}

#[test]
fn test_ws_query_defaults_when_missing() {
    let q = query(None, None, None, None);
    let parsed = q.to_spawn_params();
    // shell 缺省时 fallback 到 default_shell()（env SHELL 或 /bin/bash）
    assert!(!parsed.shell.is_empty(), "shell 应有默认值");
    assert!(parsed.args.is_empty());
    assert_eq!(parsed.cols, 80);
    assert_eq!(parsed.rows, 24);
}

#[test]
fn test_ws_query_args_split_by_whitespace() {
    let q = query(None, Some("-l  --verbose".to_string()), None, None);
    let parsed = q.to_spawn_params();
    // 多个空格应被过滤
    assert_eq!(parsed.args, vec!["-l", "--verbose"]);
}

/// Helper: construye una HeaderMap con (o sin) un Origin dado.
fn headers_with_origin(origin: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(o) = origin {
        headers.insert("origin", o.parse().unwrap());
    }
    headers
}

#[test]
fn test_origin_loopback_port_matches() {
    let a = auth("127.0.0.1", 8080, None);
    assert!(a.origin_allowed(&headers_with_origin(Some("http://127.0.0.1:8080"))));
    assert!(a.origin_allowed(&headers_with_origin(Some("http://localhost:8080"))));
}

#[test]
fn test_origin_rejects_wrong_port_and_host() {
    let a = auth("127.0.0.1", 8080, None);
    // Puerto distinto del real: mismo valor que rechaza el navegador legítimo
    // si el server quedó en otro puerto (PORT=0), y que toleraría la página ajena.
    assert!(!a.origin_allowed(&headers_with_origin(Some("http://localhost:9999"))));
    // Host no-loopback con el puerto correcto pero origen ajeno.
    assert!(!a.origin_allowed(&headers_with_origin(Some("http://evil.example:8080"))));
    assert!(!a.origin_allowed(&headers_with_origin(Some("https://localhost:8080"))));
    assert!(!a.origin_allowed(&headers_with_origin(Some("http://127.0.0.2:8080"))));
}

#[test]
fn test_origin_rejects_missing_header() {
    let a = auth("127.0.0.1", 8080, None);
    assert!(!a.origin_allowed(&headers_with_origin(None)));
}

#[test]
fn test_origin_non_loopback_host_accepted() {
    // Server expuesto en la LAN: el Origin del propio host bind pasa (el token
    // es lo que lo autentica contra terceros).
    let a = auth("192.168.1.5", 9000, Some("t0k3n"));
    assert!(a.origin_allowed(&headers_with_origin(Some("http://192.168.1.5:9000"))));
    // El Origin de una página ajena sigue rechazado a nivel de origen.
    assert!(!a.origin_allowed(&headers_with_origin(Some("http://evil.example:9000"))));
}

#[test]
fn test_token_required_when_set() {
    let a = auth("192.168.1.5", 9000, Some("t0k3n"));
    let no_tok = headers_with_origin(Some("http://192.168.1.5:9000"));
    let with_tok = {
        let mut h = no_tok.clone();
        h.insert("x-pty-token", "t0k3n".parse().unwrap());
        h
    };
    let wrong_tok = {
        let mut h = no_tok.clone();
        h.insert("x-pty-token", "n0pe".parse().unwrap());
        h
    };
    assert!(!a.token_allowed(None, &no_tok));
    assert!(!a.token_allowed(Some("n0pe"), &no_tok));
    assert!(a.token_allowed(Some("t0k3n"), &no_tok));
    assert!(a.token_allowed(None, &with_tok));
    assert!(!a.token_allowed(None, &wrong_tok));
}

#[test]
fn test_token_not_required_on_loopback() {
    let a = auth("127.0.0.1", 8080, None);
    assert!(a.token_allowed(None, &headers_with_origin(Some("http://127.0.0.1:8080"))));
}

// ─── Integración: server real + upgrade de WebSocket ─────────────────────────

use axum::http::Request as HttpRequest;

/// Levanta el server en un puerto efímero y devuelve (base_url, addr).
async fn spawn_server(host: &str, token: Option<&str>) -> (String, std::net::SocketAddr) {
    let cfg = crate::config::Config::parse_from([
        "nexum-web-pty",
        "--host",
        host,
        "--port",
        "0",
        "--shell",
        // Un shell rápido y silencioso para el upgrade aceptado.
        "/bin/true",
        "--cwd",
        "/tmp",
    ]);
    let listener = tokio::net::TcpListener::bind((host, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = crate::build_app(
        cfg,
        addr.port(),
        token.map(|t| t.to_string()),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("http://{}:{}", host, addr.port()),
        addr,
    )
}

fn upgrade_request(host_name: &str, port: u16, origin: Option<&str>, token_query: Option<&str>) -> HttpRequest<()> {
    let url_str = format!(
        "ws://{}:{}/ws{}",
        host_name,
        port,
        token_query.map(|t| format!("?token={t}")).unwrap_or_default()
    );
    let mut req = HttpRequest::builder()
        .uri(url_str)
        .header("host", format!("{}:{}", host_name, port))
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header(
            "sec-websocket-key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .unwrap();
    if let Some(origin) = origin {
        req.headers_mut().insert("origin", origin.parse().unwrap());
    }
    req
}

#[tokio::test]
async fn test_ws_upgrade_accepted_with_correct_origin() {
    let (_, addr) = spawn_server("127.0.0.1", None).await;
    let req = upgrade_request(
        "127.0.0.1",
        addr.port(),
        Some(&format!("http://127.0.0.1:{}", addr.port())),
        None,
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.expect("upgrade should be accepted");
    let _ = ws.close(None).await;
}

#[tokio::test]
async fn test_ws_upgrade_rejected_without_origin() {
    let (_, addr) = spawn_server("127.0.0.1", None).await;
    let req = upgrade_request("127.0.0.1", addr.port(), None, None);
    let err = tokio_tungstenite::connect_async(req)
        .await
        .err()
        .expect("upgrade must fail without Origin");
    let status = match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => resp.status(),
        other => panic!("expected Http error, got {other:?}"),
    };
    assert_eq!(status, 403);
}

#[tokio::test]
async fn test_ws_upgrade_rejected_with_wrong_origin() {
    let (_, addr) = spawn_server("127.0.0.1", None).await;
    let req = upgrade_request("127.0.0.1", addr.port(), Some("http://evil.example:9999"), None);
    let err = tokio_tungstenite::connect_async(req)
        .await
        .err()
        .expect("upgrade must fail with a foreign Origin");
    let status = match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => resp.status(),
        other => panic!("expected Http error, got {other:?}"),
    };
    assert_eq!(status, 403);
}

#[tokio::test]
async fn test_ws_upgrade_needs_token_on_non_loopback_host() {
    let (_, addr) = spawn_server("0.0.0.0", Some("s3cr3t-token")).await;
    // Sin token ⇒ 403.
    let req = upgrade_request("127.0.0.1", addr.port(), None, None);
    let err = tokio_tungstenite::connect_async(req)
        .await
        .err()
        .expect("upgrade must fail without token");
    let status = match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => resp.status(),
        other => panic!("expected Http error, got {other:?}"),
    };
    assert_eq!(status, 403);

    // Con token por query param ⇒ aceptado.
    let req = upgrade_request(
        "127.0.0.1",
        addr.port(),
        Some(&format!("http://0.0.0.0:{}", addr.port())),
        Some("s3cr3t-token"),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("upgrade should be accepted with the token");
    let _ = ws.close(None).await;
}
