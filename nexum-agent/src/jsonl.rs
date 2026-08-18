//! ESQUEMA ÚNICO de la traza JSONL.
//!
//! Había dos escritores al MISMO archivo con esquemas distintos:
//!
//! ```text
//! turn_log      {"ts":1785493928006,"event":"turn_routed","flow":"FULL_REACT",…}
//! metrics::emit {"ts":"2026-07-31T11:40:13.588Z","sid":…,"event":"llm.retry","data":{…}}
//! ```
//!
//! Distinto formato de timestamp, distinta forma —plano contra anidado bajo
//! `data`—, y sólo uno con `sid`/`rid`. Cuál se usaba se decidía por accidente
//! histórico, y elegir mal costó un build y un archivo vacío: `trap.cache_anomaly`
//! se emitía por un sink que en ese camino no escribía nada.
//!
//! # Cuál es autoritativo
//!
//! **Ninguno de los dos: lo es el ESQUEMA.** Los dos escritores se conservan
//! porque sus restricciones de runtime son genuinamente distintas, y esa es la
//! única razón para elegir uno u otro:
//!
//! | Escritor | Cuándo | Por qué |
//! |---|---|---|
//! | [`crate::turn_log`] | contextos sync, o sin runtime tokio vivo | append directo, sin canal ni task; funciona en cualquier lado |
//! | [`crate::metrics`] | dentro de tokio, con `sid`/`rid` a mano | canal + writer task, rotación por fecha, no bloquea |
//!
//! La regla deja de ser "cuál usaron los que estaban antes" y pasa a ser una
//! pregunta mecánica: ¿hay runtime? Si no lo hay, `turn_log`.
//!
//! # El esquema
//!
//! ```text
//! {"ts":<epoch_ms>,"event":"<nombre>","sid":"…","rid":"…","data":{…}}
//! ```
//!
//! - `ts` en **epoch ms** y no RFC3339: ordenar y restar es trivial, y un
//!   humano lo convierte cuando lo necesita. Al revés cuesta más.
//! - `data` **anidado** y no plano: los campos de un evento no pueden pisar los
//!   del sobre. Un evento con un campo `event` propio rompía el archivo.
//! - `sid`/`rid` opcionales, se omiten si no hay.
//!
//! Cero secretos: acá van decisiones de ruteo, tiempos y conteos. Nunca el
//! contenido del prompt, ni credenciales, ni la respuesta del modelo.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milisegundos desde epoch.
pub fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Escapa una cadena para incrustarla en JSON sin dependencias.
///
/// `turn_log` no usa `serde_json` a propósito: es el sink que tiene que
/// funcionar aunque todo lo demás esté roto.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Arma una línea con el esquema único. `data_json` ya viene serializado.
pub fn envelope(
    event: &str,
    sid: Option<&str>,
    rid: Option<&str>,
    data_json: &str,
) -> String {
    let mut linea = format!(r#"{{"ts":{},"event":"{}""#, now_millis(), escape(event));
    if let Some(s) = sid.filter(|s| !s.is_empty()) {
        linea.push_str(&format!(r#","sid":"{}""#, escape(s)));
    }
    if let Some(r) = rid.filter(|r| !r.is_empty()) {
        linea.push_str(&format!(r#","rid":"{}""#, escape(r)));
    }
    linea.push_str(&format!(r#","data":{data_json}}}"#));
    linea
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_sobre_no_puede_ser_pisado_por_los_datos() {
        // La razón de anidar bajo `data`: un evento con un campo `event` propio
        // producía dos claves `event` en la misma línea.
        let l = envelope("turn_end", None, None, r#"{"event":"impostor","ts":1}"#);
        let v: serde_json::Value = serde_json::from_str(&l).expect("JSON válido");
        assert_eq!(v["event"], "turn_end");
        assert_eq!(v["data"]["event"], "impostor");
    }

    #[test]
    fn sid_y_rid_se_omiten_si_no_hay() {
        let l = envelope("x", None, None, "{}");
        assert!(!l.contains("sid"), "{l}");
        assert!(!l.contains("rid"), "{l}");
    }

    #[test]
    fn sid_y_rid_aparecen_cuando_hay() {
        let l = envelope("x", Some("s1"), Some("r1"), "{}");
        let v: serde_json::Value = serde_json::from_str(&l).unwrap();
        assert_eq!(v["sid"], "s1");
        assert_eq!(v["rid"], "r1");
    }

    #[test]
    fn las_comillas_y_saltos_no_rompen_el_json() {
        let l = envelope("ev\"raro", Some("con\nsalto"), None, "{}");
        let v: serde_json::Value = serde_json::from_str(&l).expect("JSON válido");
        assert_eq!(v["event"], "ev\"raro");
        assert_eq!(v["sid"], "con\nsalto");
    }

    #[test]
    fn el_ts_es_epoch_ms_no_texto() {
        // Ordenar y restar tiene que ser trivial; RFC3339 obliga a parsear.
        let l = envelope("x", None, None, "{}");
        let v: serde_json::Value = serde_json::from_str(&l).unwrap();
        assert!(v["ts"].is_number(), "ts debe ser numérico: {l}");
    }
}
