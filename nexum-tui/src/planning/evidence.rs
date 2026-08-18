//! Writer vivo de Experience/Evidence (Fase 5). Escribe eventos REALES del
//! camino del producto a `~/.nexum/experience/evidence.jsonl` con hash chain
//! SHA-256 y redacción por construcción: JAMÁS texto crudo del usuario, solo
//! hashes. Resuelve F8 (Evidence no cableado) para el camino del plan.
//!
//! Privacidad: los campos de entrada/salida son hashes; no hay free-text.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// Lock de proceso para serializar el append (una línea íntegra por vez).
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Directorio de evidencia: override para tests > `~/.nexum/experience`.
pub fn evidence_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("NEXUM_EXPERIENCE_DIR") {
        if !d.is_empty() {
            return Some(PathBuf::from(d));
        }
    }
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".nexum").join("experience"))
}

fn evidence_path() -> Option<PathBuf> {
    evidence_dir().map(|d| d.join("evidence.jsonl"))
}

/// Hash corto y estable de un texto (para no persistir contenido crudo).
pub fn hash_text(t: &str) -> String {
    let mut h = Sha256::new();
    h.update(t.as_bytes());
    format!("{:x}", h.finalize())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Lee el `entry_hash` del último registro (para encadenar). None si no hay archivo.
fn last_hash(path: &PathBuf) -> Option<String> {
    let f = File::open(path).ok()?;
    let mut last = None;
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(h) = v.get("entry_hash").and_then(|x| x.as_str()) {
                last = Some(h.to_string());
            }
        }
    }
    last
}

/// Un evento de evidencia del ciclo de vida de una tarea real del producto.
pub struct EvidenceEvent<'a> {
    pub trace_id: &'a str,
    pub task_id: &'a str,
    pub plan_id: Option<&'a str>,
    /// task_started | route_decided | plan_generated | plan_validated |
    /// plan_rejected | plan_consumed | result_verified | task_completed | task_failed
    pub lifecycle: &'a str,
    pub component: &'a str,
    pub provenance: &'a str,
    /// Hash del input (nunca el texto). Vacío si no aplica.
    pub input_hash: &'a str,
    /// Hash del output (nunca el texto). Vacío si no aplica.
    pub output_hash: &'a str,
}

/// Persiste un evento con hash chain. Total: cualquier fallo de IO se traga
/// (la evidencia nunca debe romper el hot path del producto). Devuelve el
/// `entry_hash` escrito si tuvo éxito.
pub fn record(ev: &EvidenceEvent) -> Option<String> {
    let dir = evidence_dir()?;
    let path = evidence_path()?;
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::fs::create_dir_all(&dir).ok()?;

    let prev = last_hash(&path).unwrap_or_else(|| "genesis".to_string());
    let ts = now_ms();
    // El entry_hash encadena: prev + campos determinísticos (sin free-text).
    let chain_material = format!(
        "{prev}|{ts}|{}|{}|{}|{}|{}|{}|{}|{}",
        ev.trace_id,
        ev.task_id,
        ev.plan_id.unwrap_or(""),
        ev.lifecycle,
        ev.component,
        ev.provenance,
        ev.input_hash,
        ev.output_hash
    );
    let entry_hash = hash_text(&chain_material);

    let record = serde_json::json!({
        "schema_version": 1,
        "ts_ms": ts as u64,
        "trace_id": ev.trace_id,
        "task_id": ev.task_id,
        "plan_id": ev.plan_id,
        "lifecycle": ev.lifecycle,
        "component": ev.component,
        "provenance": ev.provenance,
        "input_hash": ev.input_hash,
        "output_hash": ev.output_hash,
        "prev_hash": prev,
        "entry_hash": entry_hash,
    });

    let mut f = OpenOptions::new().create(true).append(true).open(&path).ok()?;
    writeln!(f, "{record}").ok()?;
    Some(entry_hash)
}

/// Verifica la integridad de la cadena completa (para el gate evidence_chain_failures=0).
/// Devuelve (registros_ok, primer_fallo). Cadena vacía = íntegra.
pub fn verify_chain() -> (usize, Option<usize>) {
    let Some(path) = evidence_path() else {
        return (0, None);
    };
    let Ok(f) = File::open(&path) else {
        return (0, None);
    };
    let mut prev = "genesis".to_string();
    let mut ok = 0usize;
    for (i, line) in BufReader::new(f).lines().map_while(Result::ok).enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            return (ok, Some(i));
        };
        let rec_prev = v.get("prev_hash").and_then(|x| x.as_str()).unwrap_or("");
        let ts = v.get("ts_ms").and_then(|x| x.as_u64()).unwrap_or(0) as u128;
        let recompute = format!(
            "{rec_prev}|{ts}|{}|{}|{}|{}|{}|{}|{}|{}",
            v.get("trace_id").and_then(|x| x.as_str()).unwrap_or(""),
            v.get("task_id").and_then(|x| x.as_str()).unwrap_or(""),
            v.get("plan_id").and_then(|x| x.as_str()).unwrap_or(""),
            v.get("lifecycle").and_then(|x| x.as_str()).unwrap_or(""),
            v.get("component").and_then(|x| x.as_str()).unwrap_or(""),
            v.get("provenance").and_then(|x| x.as_str()).unwrap_or(""),
            v.get("input_hash").and_then(|x| x.as_str()).unwrap_or(""),
            v.get("output_hash").and_then(|x| x.as_str()).unwrap_or(""),
        );
        let expect = hash_text(&recompute);
        let got = v.get("entry_hash").and_then(|x| x.as_str()).unwrap_or("");
        if rec_prev != prev || got != expect {
            return (ok, Some(i));
        }
        prev = got.to_string();
        ok += 1;
    }
    (ok, None)
}

#[cfg(test)]
#[path = "evidence_test.rs"]
mod evidence_test;
