//! Cliente HTTP mínimo del MemoryGateway — mismo patrón que
//! `hormiguero::http` pero con el header del contrato de memoria
//! (`X-Nexum-Memory-Token`) y timeout distinguible para el mapeo de
//! errores. El token NUNCA se loggea.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Errores de transporte, previos al contrato.
pub enum TransportError {
    Timeout,
    Io(String),
}

pub fn request(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    json_body: Option<&str>,
    budget: Duration,
) -> Result<HttpResponse, TransportError> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let connect_budget = budget.min(Duration::from_millis(80));
    let stream = TcpStream::connect_timeout(&addr, connect_budget)
        .map_err(|e| TransportError::Io(format!("connect: {e}")))?;
    stream
        .set_read_timeout(Some(budget))
        .map_err(|e| TransportError::Io(format!("timeout cfg: {e}")))?;
    stream
        .set_write_timeout(Some(budget))
        .map_err(|e| TransportError::Io(format!("timeout cfg: {e}")))?;
    let mut stream = stream;

    let body = json_body.unwrap_or("");
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(tok) = token {
        req.push_str(&format!("X-Nexum-Memory-Token: {tok}\r\n"));
    }
    if json_body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));

    stream
        .write_all(req.as_bytes())
        .map_err(|e| TransportError::Io(format!("write: {e}")))?;

    let mut raw = Vec::with_capacity(4096);
    stream.read_to_end(&mut raw).map_err(|e| {
        if matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            TransportError::Timeout
        } else {
            TransportError::Io(format!("read: {e}"))
        }
    })?;
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<HttpResponse, TransportError> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| TransportError::Io("respuesta HTTP malformada".into()))?;
    let status: u16 = head
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| TransportError::Io("status line inválida".into()))?;
    let content_len = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if k.eq_ignore_ascii_case("content-length") {
            v.trim().parse::<usize>().ok()
        } else {
            None
        }
    });
    let body = match content_len {
        Some(n) if n <= body.len() => &body[..n],
        _ => body,
    };
    Ok(HttpResponse {
        status,
        body: body.to_string(),
    })
}
