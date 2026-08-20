use axum::http::HeaderMap;
use axum::http::header;

/// Auth del endpoint /ws: validación de origen y token opcional.
///
/// - `port` es el puerto **real** que quedó asignado al listener (resuelto
///   con `local_addr()` tras bindear), no un valor fijo: si el server arranca
///   con `PORT=0` (puerto efímero), el browser manda el Origin del puerto
///   asignado, que solo conocemos después del bind.
/// - `token` es `Some(...)` únicamente cuando el server escucha en un host
///   distinto de loopback (ver `is_loopback_host`): host no-loopback implica
///   que el proceso puede estar expuesto a la red local, y ahí un Origin check
///   solo no alcanza (cualquier página de la LAN puede forjar ese Origin).
#[derive(Debug, Clone)]
pub struct WsAuth {
    pub host: String,
    pub port: u16,
    pub token: Option<String>,
}

impl WsAuth {
    pub fn new(host: String, port: u16, token: Option<String>) -> Self {
        Self { host, port, token }
    }

    /// El cliente viene del propio server (mismo puerto real) o, si el server
    /// escucha en un host no-loopback, del Origin de ese mismo host.
    /// WS no pasa por same-origin policy, así que una página ajena abierta en
    /// el browser podría scriptear `new WebSocket("ws://<host>/ws")`: este
    /// check la descarta antes del upgrade. Ausencia de Origin ⇒ rechazo.
    pub fn origin_allowed(&self, headers: &HeaderMap) -> bool {
        let Some(origin) = headers
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
        else {
            return false;
        };
        origin == format!("http://localhost:{}", self.port)
            || origin == format!("http://127.0.0.1:{}", self.port)
            || (!is_loopback_host(&self.host) && origin == format!("http://{}:{}", self.host, self.port))
    }

    /// Cuando hay token (host no-loopback), el cliente debe presentarlo:
    /// query param `token` o header `x-pty-token`. Sin token activo, siempre true.
    pub fn token_allowed(&self, query_token: Option<&str>, headers: &HeaderMap) -> bool {
        let Some(expected) = self.token.as_deref() else {
            return true;
        };
        let provided = query_token
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                headers
                    .get("x-pty-token")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            });
        matches!(provided.as_deref(), Some(got) if got == expected)
    }
}

/// Hosts que son loopback: en ellos no se exige token (el default del server).
pub fn is_loopback_host(host: &str) -> bool {
    host == "127.0.0.1" || host == "localhost" || host == "::1"
}