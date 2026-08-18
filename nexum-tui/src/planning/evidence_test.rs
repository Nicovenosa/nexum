// ALLOW justificado: temporal de test con PID en el nombre. No hay recurso compartido entre
// procesos, que es lo que el lint protege.
#![allow(clippy::disallowed_methods)]

//! Tests del writer de Evidence: hash chain, redacción, verificación.

use super::*;

fn with_tempdir<F: FnOnce()>(f: F) {
    let _guard = crate::hormiguero::bridge::test_env_lock();
    let dir = std::env::temp_dir().join(format!(
        "nexum-ev-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::env::set_var("NEXUM_EXPERIENCE_DIR", &dir);
    f();
    std::env::remove_var("NEXUM_EXPERIENCE_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_hash_determinista_y_sin_texto_crudo() {
    assert_eq!(hash_text("hola"), hash_text("hola"));
    assert_ne!(hash_text("hola"), hash_text("chau"));
    // 64 hex chars (SHA-256).
    assert_eq!(hash_text("x").len(), 64);
}

#[test]
fn test_record_encadena_y_verifica() {
    with_tempdir(|| {
        let h1 = record(&EvidenceEvent {
            trace_id: "t1",
            task_id: "task-a",
            plan_id: None,
            lifecycle: "task_started",
            component: "tui",
            provenance: "test",
            input_hash: &hash_text("pedido"),
            output_hash: "",
        });
        let h2 = record(&EvidenceEvent {
            trace_id: "t1",
            task_id: "task-a",
            plan_id: Some("plan-x"),
            lifecycle: "plan_generated",
            component: "planning-gateway",
            provenance: "test",
            input_hash: "",
            output_hash: "",
        });
        assert!(h1.is_some() && h2.is_some(), "ambos registros escriben");
        assert_ne!(h1, h2, "hashes distintos");
        let (ok, fail) = verify_chain();
        assert_eq!(ok, 2, "dos registros íntegros");
        assert!(fail.is_none(), "cadena sin fallos");
    });
}

#[test]
fn test_cadena_detecta_corrupcion() {
    with_tempdir(|| {
        record(&EvidenceEvent {
            trace_id: "t2",
            task_id: "task-b",
            plan_id: None,
            lifecycle: "task_started",
            component: "tui",
            provenance: "test",
            input_hash: "",
            output_hash: "",
        });
        // Corromper el archivo append-eando una línea con entry_hash falso.
        let path = evidence_dir().unwrap().join("evidence.jsonl");
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str(
            "{\"prev_hash\":\"mentira\",\"entry_hash\":\"falso\",\"ts_ms\":0,\"trace_id\":\"x\"}\n",
        );
        std::fs::write(&path, content).unwrap();
        let (_ok, fail) = verify_chain();
        assert!(fail.is_some(), "detecta la línea corrupta");
    });
}
