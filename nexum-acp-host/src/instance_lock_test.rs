// ALLOW justificado: temporal de test con PID/UUID; sin recurso compartido.
#![allow(clippy::disallowed_methods)]

//! Tests del guard de instancia única (flock).

use super::*;

fn tmp_socket() -> PathBuf {
    std::env::temp_dir().join(format!(
        "nexum-acp-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn test_lock_path_agrega_sufijo() {
    let p = lock_path_for(Path::new("/run/user/1000/nexum/acp.sock"));
    assert_eq!(p, PathBuf::from("/run/user/1000/nexum/acp.sock.lock"));
}

#[test]
fn test_instancia_unica_segundo_intento_none() {
    let sock = tmp_socket();
    let first = acquire(&sock).expect("io").expect("primer host toma el lock");
    // Un segundo intento sobre el mismo socket, con el primero vivo ⇒ None.
    let second = acquire(&sock).expect("io");
    assert!(second.is_none(), "el segundo host NO arranca (instancia única)");
    let lock_file = first.path().to_path_buf();
    drop(first);
    // Liberado el lock ⇒ un nuevo host puede tomarlo.
    let third = acquire(&sock).expect("io");
    assert!(third.is_some(), "tras liberar, un nuevo host puede tomar el lock");
    drop(third);
    let _ = std::fs::remove_file(&lock_file);
    let _ = std::fs::remove_file(&sock);
}

#[test]
fn test_pid_registrado_en_el_lock() {
    let sock = tmp_socket();
    let guard = acquire(&sock).expect("io").expect("lock");
    let content = std::fs::read_to_string(guard.path()).expect("leer lock");
    assert_eq!(content.trim(), std::process::id().to_string(), "PID propio registrado");
    drop(guard);
    let _ = std::fs::remove_file(lock_path_for(&sock));
    let _ = std::fs::remove_file(&sock);
}
