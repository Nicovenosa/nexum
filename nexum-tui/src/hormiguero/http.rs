//! Mini cliente HTTP/1.1 sobre TcpStream, exclusivo para el sidecar loopback.
//!
//! Por qué no reqwest acá: el pre-route corre en el path sync de
//! `submit_message`; este cliente da timeouts duros nativos (connect/read/
//! write) sin tocar el runtime async ni sumar dependencias. Solo habla con
//! 127.0.0.1 — jamás se usa para red externa.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Request mínima a loopback con presupuesto TOTAL en ms.
/// El token viaja en X-Nexum-Token y NUNCA se loggea.
pub fn request(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    json_body: Option<&str>,
    budget: Duration,
) -> Result<HttpResponse, String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let connect_budget = budget.min(Duration::from_millis(80));
    let stream =
        TcpStream::connect_timeout(&addr, connect_budget).map_err(|e| format!("connect: {e}"))?;
    stream
        .set_read_timeout(Some(budget))
        .map_err(|e| format!("timeout cfg: {e}"))?;
    stream
        .set_write_timeout(Some(budget))
        .map_err(|e| format!("timeout cfg: {e}"))?;
    let mut stream = stream;

    let body = json_body.unwrap_or("");
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(tok) = token {
        req.push_str(&format!("X-Nexum-Token: {tok}\r\n"));
    }
    if json_body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));

    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write: {e}"))?;

    let mut raw = Vec::with_capacity(4096);
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read: {e}"))?;
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "respuesta HTTP malformada".to_string())?;
    let status_line = head.lines().next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "status line inválida".to_string())?;
    // Connection: close + read_to_end ⇒ el body llega completo; se toma
    // Content-Length si está para recortar posibles bytes extra.
    let content_len = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.eq_ignore_ascii_case("content-length") {
                v.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(body.len());
    let body = body.as_bytes();
    let body = &body[..content_len.min(body.len())];
    Ok(HttpResponse {
        status,
        body: String::from_utf8_lossy(body).into_owned(),
    })
}

#[cfg(test)]
#[path = "http_test.rs"]
mod http_test;
