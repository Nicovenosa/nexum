use super::super::http_testutil::mock_route_server;
use super::*;
use std::net::TcpListener;

#[test]
fn test_request_parsea_status_y_body() {
    let port = mock_route_server(200, r#"{"ok":true}"#, 0);
    let resp = request(
        port,
        "GET",
        "/health",
        None,
        None,
        std::time::Duration::from_millis(500),
    )
    .expect("request ok");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, r#"{"ok":true}"#);
}

#[test]
fn test_request_timeout_con_servidor_lento() {
    let port = mock_route_server(200, r#"{"ok":true}"#, 2000);
    let start = std::time::Instant::now();
    let resp = request(
        port,
        "GET",
        "/health",
        None,
        None,
        std::time::Duration::from_millis(200),
    );
    assert!(resp.is_err(), "servidor lento debe dar timeout");
    assert!(
        start.elapsed() < std::time::Duration::from_millis(1500),
        "el timeout corta antes del delay del servidor"
    );
}

#[test]
fn test_request_puerto_cerrado_falla_rapido() {
    // Puerto casi seguro cerrado (bind efímero y drop inmediato).
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let start = std::time::Instant::now();
    let resp = request(
        port,
        "GET",
        "/health",
        None,
        None,
        std::time::Duration::from_millis(800),
    );
    assert!(resp.is_err());
    assert!(
        start.elapsed() < std::time::Duration::from_millis(300),
        "connection refused en loopback es inmediato"
    );
}
