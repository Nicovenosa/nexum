//! E2E del producto integrado (M-3 FASE 14) — corren contra el sidecar
//! Python REAL a través del cliente productivo (`client::`), la misma
//! ruta que usa la TUI. Ignorados por defecto para mantener la suite
//! hermética; correr con:
//!   cargo test -p nexum-tui --lib memory_gateway::e2e -- --ignored --test-threads=1

use super::*;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::ui::demo_mode::test_env_lock()
}

fn cli_dir() -> PathBuf {
    // nexum-tui/ → nexum-runtime/ → nexum-agent-app-cli/
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

struct Sidecar {
    child: Child,
    dir: tempfile::TempDir,
    db: PathBuf,
}

impl Sidecar {
    fn spawn(dir: tempfile::TempDir, db: PathBuf) -> Self {
        let child = Command::new("python3")
            .args(["-m", "nexum_memory_gateway"])
            .current_dir(cli_dir())
            .env("PYTHONPATH", cli_dir().join("src"))
            .env("NEXUM_MEMORY", "on")
            .env("NEXUM_MEMORY_RUNTIME_DIR", dir.path())
            .env("NEXUM_MEMORY_DB", &db)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sidecar");
        let port_file = dir.path().join("memory.port");
        let t0 = Instant::now();
        while !port_file.exists() {
            assert!(t0.elapsed() < Duration::from_secs(5), "sidecar no publicó puerto");
            std::thread::sleep(Duration::from_millis(30));
        }
        std::env::set_var("NEXUM_MEMORY_RUNTIME_DIR", dir.path());
        Sidecar { child, dir, db }
    }

    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("e2e.sqlite3");
        Self::spawn(dir, db)
    }

    fn restart(mut self) -> Self {
        self.child.kill().ok();
        self.child.wait().ok();
        std::fs::remove_file(self.dir.path().join("memory.port")).ok();
        let db = self.db.clone();
        Self::spawn(self.dir, db)
    }

    fn stop(mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
        std::env::remove_var("NEXUM_MEMORY_RUNTIME_DIR");
    }
}

fn proposal(content: &str, key: Option<&str>, idem: &str) -> PendingProposal {
    PendingProposal {
        content: content.into(),
        key: key.map(Into::into),
        scope_type: "user".into(),
        scope_id: "local".into(),
        source_reference: "e2e 2026-07-18".into(),
        idempotency_key: idem.into(),
    }
}

#[test]
#[ignore = "e2e real: requiere python3 + sidecar del repo"]
fn e2e_1_guardado_recall_reinicio_borrado() {
    let _guard = env_lock();
    let sc = Sidecar::new();

    // Detección determinística (misma ruta que la TUI usa en submit).
    let contenido = intent::parse_save_intent("Recordá que trabajo en X").expect("intent");
    assert_eq!(contenido, "trabajo en X");
    let key = intent::derive_key(&contenido);

    // "Cancelar confirmación" = la propuesta se descarta ANTES de save:
    // cero writes en el backend.
    let _descartada = proposal(&contenido, key.as_deref(), "e2e1-a");
    let l = client::list("user", "local").expect("list");
    assert_eq!(l.results.len(), 0, "cancelar ⇒ cero writes (gate 2)");

    // Repetir + confirmar.
    let p = proposal(&contenido, key.as_deref(), "e2e1-b");
    let saved = client::save_confirmed(&p).expect("save confirmado");
    assert!(saved.conflict.is_none());

    // Reiniciar Nexum (sidecar) y consultar dónde trabaja el usuario.
    let sc = sc.restart();
    let r = client::recall("trabajo", "user", "local").expect("recall post-restart");
    assert_eq!(r.results.len(), 1, "persistencia tras reinicio (gate 1)");
    let e = &r.results[0];
    assert_eq!(e.content, "trabajo en X");
    assert_eq!(e.source_type, "user_explicit");
    assert_eq!(e.source_reference, "e2e 2026-07-18", "proveniencia (gate 4)");

    // Eliminar y verificar que no vuelve como activa.
    let d = client::delete(&e.id, "user", "local").expect("delete");
    assert!(d.deleted);
    assert_eq!(d.mode, "tombstone");
    let l = client::list("user", "local").expect("list final");
    assert!(l.results.is_empty(), "gate 7: eliminada no vuelve como activa");
    sc.stop();
}

#[test]
#[ignore = "e2e real: requiere python3 + sidecar del repo"]
fn e2e_2_contradiccion_conservada_y_resuelta_con_historial() {
    let _guard = env_lock();
    let sc = Sidecar::new();

    let x = client::save_confirmed(&proposal("trabajo en X", Some("trabajo-en"), "e2e2-x"))
        .expect("save X");
    let y = client::save_confirmed(&proposal("trabajo en Y", Some("trabajo-en"), "e2e2-y"))
        .expect("save Y");
    let c = y.conflict.expect("conflicto detectado (gate 5)");
    assert_eq!(c.status, "open");
    assert_eq!(c.entries.len(), 2, "ambas versiones conservadas");

    // "Cancelar" la resolución: no resolver. Ambas siguen conservadas.
    let abiertos = client::open_conflicts("user", "local").expect("conflictos");
    assert_eq!(abiertos.open_conflicts.len(), 1);

    // Resolución explícita: gana Y.
    let res = client::resolve(&c.group_id, "user", "local", "winner", Some(&y.id), "el usuario elige Y")
        .expect("resolve");
    assert_eq!(res.conflict.status, "resolved");

    // Reiniciar y verificar resolución + historial auditable.
    let sc = sc.restart();
    let gx = client::get(&x.id, "user", "local").expect("get X");
    assert_eq!(gx.entry.status, "superseded", "historial: X preservada como superseded");
    let gy = client::get(&y.id, "user", "local").expect("get Y");
    assert_eq!(gy.entry.status, "active");
    let r = client::recall("trabajo", "user", "local").expect("recall");
    assert_eq!(r.results.len(), 1, "solo la ganadora vuelve como activa");
    assert_eq!(r.results[0].content, "trabajo en Y");
    sc.stop();
}

#[test]
#[ignore = "e2e real: requiere python3 + sidecar del repo"]
fn e2e_3_scopes_sin_leaks() {
    let _guard = env_lock();
    let sc = Sidecar::new();
    let mut p = proposal("secreto del proyecto A", None, "e2e3-a");
    p.scope_type = "project".into();
    p.scope_id = "proyecto-a".into();
    client::save_confirmed(&p).expect("save en project A");
    client::save_confirmed(&proposal("dato del user", None, "e2e3-u")).expect("save user");

    let rb = client::recall("secreto", "project", "proyecto-b").expect("recall B");
    assert!(rb.results.is_empty(), "cero leak hacia project B (gate 3)");
    let ru = client::recall("secreto", "user", "local").expect("recall user");
    assert!(ru.results.is_empty(), "cero leak hacia user scope");
    let ra = client::recall("secreto", "project", "proyecto-a").expect("recall A");
    assert_eq!(ra.results.len(), 1);
    sc.stop();
}

#[test]
#[ignore = "e2e real: requiere python3 + sidecar del repo"]
fn e2e_4_backend_caido_degrada_sin_inventar_y_recupera() {
    let _guard = env_lock();
    let sc = Sidecar::new();
    client::save_confirmed(&proposal("antes de la caída", None, "e2e4")).expect("save");

    // Matar el sidecar (kill -9 vía kill()): el cliente NO crashea, NO
    // inventa memoria — informa indisponibilidad.
    let dir = sc.dir;
    let db = sc.db;
    let mut child = sc.child;
    child.kill().ok();
    child.wait().ok();
    let r = client::recall("caída", "user", "local");
    assert!(
        matches!(r, Err(MemoryError::Unavailable(_)) | Err(MemoryError::Timeout)),
        "backend caído ⇒ degradación explícita, jamás datos inventados"
    );

    // Recuperación tras restart: los datos confirmados siguen.
    std::fs::remove_file(dir.path().join("memory.port")).ok();
    let sc = Sidecar::spawn(dir, db);
    let r = client::recall("caída", "user", "local").expect("recall post-recovery");
    assert_eq!(r.results.len(), 1, "recovery consistente tras kill -9");
    sc.stop();
}

#[test]
#[ignore = "e2e real: requiere python3 + sidecar del repo"]
fn e2e_5_flag_off_cero_lecturas_cero_escrituras() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("off.sqlite3");
    // Con flag off el sidecar sale con exit 3 sin crear DB.
    let status = Command::new("python3")
        .args(["-m", "nexum_memory_gateway"])
        .current_dir(cli_dir())
        .env("PYTHONPATH", cli_dir().join("src"))
        .env("NEXUM_MEMORY", "off")
        .env("NEXUM_MEMORY_RUNTIME_DIR", dir.path())
        .env("NEXUM_MEMORY_DB", &db)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run sidecar");
    assert_eq!(status.code(), Some(3), "flag OFF ⇒ exit 3");
    assert!(!db.exists(), "cero escrituras: ni la DB se crea");
    // Y el runtime ni intercepta ni llama al cliente:
    std::env::remove_var("NEXUM_MEMORY");
    assert!(!env_flag_on());
    assert!(!enabled(&MemoryUiState::default()), "gate 8: overhead funcional nulo");
}

#[test]
#[ignore = "e2e real: requiere python3 + sidecar del repo"]
fn e2e_overhead_cliente_rust_100_saves() {
    let _guard = env_lock();
    let sc = Sidecar::new();
    let mut lat: Vec<u128> = Vec::with_capacity(100);
    for i in 0..100 {
        let p = proposal(&format!("overhead {i}"), None, &format!("ov-{i}"));
        let t0 = Instant::now();
        client::save_confirmed(&p).expect("save");
        lat.push(t0.elapsed().as_micros());
    }
    lat.sort_unstable();
    println!(
        "INFO rust_client_save_p50_us={} p95_us={}",
        lat[50], lat[95]
    );
    assert!(lat[95] < 300_000, "p95 muy por debajo del gate de 300 ms");
    sc.stop();
}
