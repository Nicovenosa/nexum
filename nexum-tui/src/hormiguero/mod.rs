//! HormigueroBridge (Sprint 0) — pasillo runtime hacia el sidecar Python.
//!
//! Reglas duras: no bloquear la TUI (timeouts estrictos + breaker), fallback
//! total (sidecar caído ⇒ passthrough exacto al flujo actual), nunca exponer
//! token/reasoning interno/modelo interno, flag OFF por defecto.

pub mod acp_adapter;
pub mod bridge;
pub mod fastpath;
pub mod http;
pub mod types;

pub use bridge::{bridge, Bridge};
pub use fastpath::{classify as classify_fast, FastVerdict, StableFastVerdict};
pub use types::*;

/// Flag maestro del pasillo. Default OFF. `NEXUM_PUBLIC_DEMO=1` lo fuerza
/// OFF siempre (defensa en profundidad, mismo patrón que predictions).
pub fn hormiguero_enabled() -> bool {
    if crate::ui::demo_mode::public_demo_enabled() {
        return false;
    }
    matches!(
        std::env::var("NEXUM_HORMIGUERO")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "on" | "1" | "true" | "yes"
    )
}

/// Debug dev-only: muestra detalles internos (modelo interno, engine) en
/// /hormiguero status. Nunca activo en public demo.
pub fn hormiguero_debug_enabled() -> bool {
    !crate::ui::demo_mode::public_demo_enabled()
        && std::env::var("NEXUM_HORMIGUERO_DEBUG").as_deref() == Ok("1")
}

/// Servidor HTTP mock compartido por los tests del pasillo.
#[cfg(test)]
pub(crate) mod http_testutil {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    /// Sirve UNA respuesta enlatada y termina. Devuelve el puerto.
    /// `delay_ms` simula un sidecar lento (para tests de timeout).
    pub(crate) fn mock_route_server(status: u16, body: &str, delay_ms: u64) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let port = listener.local_addr().unwrap().port();
        let body = body.to_string();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                if delay_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                }
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        port
    }
}

#[cfg(test)]
mod flag_tests {
    use super::*;

    #[test]
    fn test_flag_off_por_defecto_y_public_demo_gana() {
        // Serializado vía env vars temporales; el resto de la matriz vive en
        // bridge_test (evita carreras de env en tests paralelos usando lock).
        let _guard = crate::hormiguero::bridge::test_env_lock();
        std::env::remove_var("NEXUM_HORMIGUERO");
        std::env::remove_var("NEXUM_PUBLIC_DEMO");
        assert!(!hormiguero_enabled(), "default OFF");
        std::env::set_var("NEXUM_HORMIGUERO", "on");
        assert!(hormiguero_enabled(), "on explícito activa");
        std::env::set_var("NEXUM_PUBLIC_DEMO", "1");
        assert!(!hormiguero_enabled(), "public demo fuerza OFF");
        std::env::remove_var("NEXUM_PUBLIC_DEMO");
        std::env::remove_var("NEXUM_HORMIGUERO");
    }
}
