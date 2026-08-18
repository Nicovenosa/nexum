// ALLOW justificado: temporal de test con PID en el nombre. No hay recurso compartido entre
// procesos, que es lo que el lint protege.
#![allow(clippy::disallowed_methods)]

use super::*;

#[test]
fn test_afirmativo_negativo() {
    for a in ["sí", "si", "dale", "confirmá", "sí creala", "ok", "adelante"] {
        assert!(is_affirmative(a), "afirmativo: {a}");
    }
    for n in ["no", "cancelá", "mejor no", "no gracias"] {
        assert!(is_negative(n), "negativo: {n}");
    }
    assert!(!is_affirmative("creá una carpeta"));
    assert!(!is_negative("sí dale"));
}

#[test]
fn test_save_load_clear_roundtrip() {
    let d = std::env::temp_dir().join(format!("pend-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let a = LocalAction {
        kind: LocalActionKind::CreateDirectory,
        name: "Prueba".into(),
        base: d.clone(),
        base_label: "test".into(),
    };
    save(&d, &a).unwrap();
    let p = load(&d).expect("pendiente cargada");
    assert_eq!(p.name, "Prueba");
    assert_eq!(p.fingerprint, a.fingerprint(), "fingerprint persiste (binding)");
    clear(&d);
    assert!(load(&d).is_none(), "clear elimina la pendiente");
    std::fs::remove_dir_all(&d).ok();
}
