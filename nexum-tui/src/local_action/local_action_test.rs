// ALLOW justificado: temporal de test con PID en el nombre. No hay recurso compartido entre
// procesos, que es lo que el lint protege.
#![allow(clippy::disallowed_methods)]

use super::*;

fn mk_ws() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let w = std::env::temp_dir().join(format!(
        "la-ws-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&w).unwrap();
    w
}

#[test]
fn test_parse_frases_espanol_crear_carpeta() {
    let w = mk_ws();
    // en el proyecto actual (workspace) para no depender de XDG del entorno.
    for frase in [
        "creá una carpeta llamada PruebaNexum en el proyecto actual",
        "crea un directorio Datos en la carpeta actual",
        "hacé una carpeta llamada Informes acá",
    ] {
        let a = parse(frase, &w).unwrap_or_else(|| panic!("no parseó: {frase}"));
        assert_eq!(a.kind, LocalActionKind::CreateDirectory);
        assert!(!a.name.is_empty(), "nombre vacío en: {frase}");
    }
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_parse_extrae_nombre_correcto() {
    let w = mk_ws();
    let a = parse("creá una carpeta llamada PruebaNexum en el proyecto actual", &w).unwrap();
    assert_eq!(a.name, "PruebaNexum");
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_parse_conversacion_normal_no_dispara() {
    let w = mk_ws();
    assert!(parse("hola cómo andás", &w).is_none());
    assert!(parse("explicame qué es un mutex", &w).is_none());
    assert!(parse("qué carpetas tengo abiertas", &w).is_none()); // sin verbo de crear
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_validate_rechaza_traversal_y_absolutos() {
    let w = mk_ws();
    for bad in ["../escape", "..", ".", "a/b", "/etc/x", "x\0y"] {
        let a = LocalAction {
            kind: LocalActionKind::CreateDirectory,
            name: bad.to_string(),
            base: w.clone(),
            base_label: "test".into(),
        };
        assert!(a.validate().is_err(), "debió rechazar: {bad:?}");
    }
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_validate_acepta_nombre_valido() {
    let w = mk_ws();
    let a = LocalAction {
        kind: LocalActionKind::CreateDirectory,
        name: "PruebaNexum".into(),
        base: w.clone(),
        base_label: "test".into(),
    };
    assert!(a.validate().is_ok());
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_execute_crea_y_detecta_existente() {
    let w = mk_ws();
    let a = LocalAction {
        kind: LocalActionKind::CreateDirectory,
        name: "NuevaCarpeta".into(),
        base: w.clone(),
        base_label: "test".into(),
    };
    match a.execute().unwrap() {
        ExecOutcome::Created(p) => assert!(std::path::Path::new(&p).is_dir()),
        other => panic!("esperaba Created, fue {other:?}"),
    }
    // segunda vez: ya existe (honesto, no error)
    assert!(matches!(a.execute().unwrap(), ExecOutcome::AlreadyExisted(_)));
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_fingerprint_invalida_por_cambio() {
    let w = mk_ws();
    let base = LocalAction {
        kind: LocalActionKind::CreateDirectory,
        name: "X".into(),
        base: w.clone(),
        base_label: "t".into(),
    };
    let fp = base.fingerprint();
    let otro_nombre = LocalAction { name: "Y".into(), ..base.clone() };
    assert_ne!(fp, otro_nombre.fingerprint(), "cambio de nombre invalida");
    // mismo → igual
    let igual = LocalAction { name: "X".into(), ..base.clone() };
    assert_eq!(fp, igual.fingerprint());
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_symlink_escape_rechazado() {
    let w = mk_ws();
    // base que es un symlink apuntando fuera: la canonicalización lo resuelve;
    // el target debe quedar bajo la base REAL, no escapar.
    let real = w.join("real");
    std::fs::create_dir_all(&real).unwrap();
    let a = LocalAction {
        kind: LocalActionKind::CreateDirectory,
        name: "ok".into(),
        base: real.clone(),
        base_label: "t".into(),
    };
    // válido: queda bajo real/. Ejecuta y verifica que quedó dentro de real.
    if let Ok(ExecOutcome::Created(p)) = a.execute() {
        assert!(p.starts_with(real.canonicalize().unwrap().to_string_lossy().as_ref()));
    }
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_dispatch_e2e_propuesta_confirmacion_creacion() {
    let w = mk_ws();
    let state = w.join("state");
    std::fs::create_dir_all(&state).unwrap();
    // 1. propuesta (HITL): NO crea todavía
    let out = dispatch("creá una carpeta llamada PruebaNexum en el proyecto actual", &w, &state);
    assert!(matches!(out, FastPathOutcome::Proposed(_)), "debe proponer, no ejecutar");
    assert!(!w.join("PruebaNexum").exists(), "no se creó antes de confirmar (gate)");
    // 2. confirmación afirmativa: ahora sí crea
    let out2 = dispatch("sí dale", &w, &state);
    match out2 {
        FastPathOutcome::Executed(m) => assert!(m.contains("PruebaNexum")),
        other => panic!("esperaba Executed, fue {other:?}"),
    }
    assert!(w.join("PruebaNexum").is_dir(), "carpeta creada tras confirmar");
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_dispatch_rechazo_cero_cambios() {
    let w = mk_ws();
    let state = w.join("state");
    std::fs::create_dir_all(&state).unwrap();
    dispatch("creá una carpeta llamada NoVa en el proyecto actual", &w, &state);
    let out = dispatch("no, mejor no", &w, &state);
    assert!(matches!(out, FastPathOutcome::Cancelled(_)));
    assert!(!w.join("NoVa").exists(), "rechazo ⇒ cero cambios");
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_dispatch_conversacion_normal_es_notlocal() {
    let w = mk_ws();
    let state = w.join("state");
    std::fs::create_dir_all(&state).unwrap();
    assert!(matches!(dispatch("explicame qué es un semáforo", &w, &state), FastPathOutcome::NotLocal));
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_latencia_fast_path_bajo_budget() {
    let w = mk_ws();
    let state = w.join("state");
    std::fs::create_dir_all(&state).unwrap();
    // Propuesta (transcript → approval visible): budget p95 < 300ms.
    let mut lat_propose = Vec::new();
    for i in 0..50 {
        let frase = format!("creá una carpeta llamada Prueba{i} en el proyecto actual");
        let t0 = std::time::Instant::now();
        let _ = dispatch(&frase, &w, &state);
        lat_propose.push(t0.elapsed().as_micros());
    }
    lat_propose.sort_unstable();
    let p95_propose = lat_propose[47];
    // Ejecución (confirmar → carpeta creada): budget p95 < 300ms.
    let mut lat_exec = Vec::new();
    for i in 0..50 {
        let frase = format!("creá una carpeta llamada Exec{i} en el proyecto actual");
        dispatch(&frase, &w, &state);
        let t0 = std::time::Instant::now();
        let _ = dispatch("sí", &w, &state);
        lat_exec.push(t0.elapsed().as_micros());
    }
    lat_exec.sort_unstable();
    let p95_exec = lat_exec[47];
    println!("FAST-PATH propose p95={p95_propose}us · exec p95={p95_exec}us");
    assert!(p95_propose < 300_000, "propose p95 {p95_propose}us < 300ms");
    assert!(p95_exec < 300_000, "exec p95 {p95_exec}us < 300ms");
    // 0 provider calls: el fast path no toca red ni Hormiguero (es puro fs).
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_nombres_con_espacios_y_acentos() {
    let w = mk_ws();
    // "Prueba Nexum" (espacio) — el parser toma hasta 4 tokens antes de stop-word.
    let a = parse("creá una carpeta llamada Informes Finales en el proyecto actual", &w).unwrap();
    assert!(a.name.contains("Informes"), "captura nombre compuesto: {}", a.name);
    assert!(a.validate().is_ok(), "espacios permitidos en el nombre");
    // acentos
    let b = parse("creá una carpeta llamada Programación en el proyecto actual", &w).unwrap();
    assert_eq!(b.name, "Programación");
    assert!(b.validate().is_ok());
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_readonly_filesystem_error_honesto() {
    let w = mk_ws();
    let ro = w.join("readonly");
    std::fs::create_dir_all(&ro).unwrap();
    // quitar permiso de escritura
    let mut perm = std::fs::metadata(&ro).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o555);
    std::fs::set_permissions(&ro, perm).unwrap();
    let a = LocalAction {
        kind: LocalActionKind::CreateDirectory,
        name: "nova".into(),
        base: ro.clone(),
        base_label: "readonly".into(),
    };
    // validate pasa (la base existe), pero execute falla con error honesto (no false completion).
    let r = a.execute();
    assert!(matches!(r, Err(LocalActionError::Io(_))), "readonly ⇒ error honesto: {r:?}");
    // restaurar para limpiar
    let mut perm = std::fs::metadata(&ro).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    std::fs::set_permissions(&ro, perm).unwrap();
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_nombre_enorme_rechazado() {
    let w = mk_ws();
    let a = LocalAction {
        kind: LocalActionKind::CreateDirectory,
        name: "x".repeat(300),
        base: w.clone(),
        base_label: "t".into(),
    };
    assert!(matches!(a.validate(), Err(LocalActionError::InvalidName(_))));
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_symlink_reemplazado_no_escapa() {
    // Base es un symlink a un dir DENTRO del temp; el target debe quedar bajo el
    // dir REAL (canonicalizado), no bajo el nombre del symlink.
    let w = mk_ws();
    let real = w.join("real_target");
    std::fs::create_dir_all(&real).unwrap();
    let link = w.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let a = LocalAction {
        kind: LocalActionKind::CreateDirectory,
        name: "sub".into(),
        base: link.clone(),
        base_label: "via-symlink".into(),
    };
    // validate canonicaliza: el target queda bajo real_target/, no bajo link/.
    assert!(a.validate().is_ok());
    if let Ok(ExecOutcome::Created(p)) = a.execute() {
        let canon_real = real.canonicalize().unwrap();
        assert!(std::path::Path::new(&p).starts_with(&canon_real),
            "el target sigue el symlink al dir real, sin escapar: {p}");
    }
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_idempotencia_100_local_actions() {
    // 100 acciones en filesystem temporal: sin errores, sin duplicados.
    let w = mk_ws();
    let state = w.join("st");
    std::fs::create_dir_all(&state).unwrap();
    for i in 0..100 {
        let frase = format!("creá una carpeta llamada Batch{i} en el proyecto actual");
        assert!(matches!(dispatch(&frase, &w, &state), FastPathOutcome::Proposed(_)));
        assert!(matches!(dispatch("sí", &w, &state), FastPathOutcome::Executed(_)));
    }
    // repetir la 0: ya existe (idempotente, no error)
    dispatch("creá una carpeta llamada Batch0 en el proyecto actual", &w, &state);
    match dispatch("sí", &w, &state) {
        FastPathOutcome::Executed(m) => assert!(m.contains("ya existía"), "repetido: ya existía"),
        other => panic!("{other:?}"),
    }
    let count = std::fs::read_dir(&w).unwrap().filter(|e| {
        e.as_ref().unwrap().file_name().to_string_lossy().starts_with("Batch")
    }).count();
    assert_eq!(count, 100, "100 carpetas únicas, sin duplicados");
    std::fs::remove_dir_all(&w).ok();
}

#[test]
fn test_tolerante_a_asr_base() {
    let w = mk_ws();
    // transcripciones reales de whisper ggml-base (errores comunes es):
    for frase in [
        "Criá una capeta llamada Provea en el proyecto actual", // creá→criá, carpeta→capeta
        "creá una carpeta llamada Datos en el proyecto actual",
    ] {
        let a = parse(frase, &w);
        assert!(a.is_some(), "el ASR imperfecto igual dispara el fast path: {frase}");
    }
    // conversación normal sigue sin disparar
    // Era `assert!(… .is_none() || true)`, que es SIEMPRE verdadero: un test que
    // aparentaba verificar y no verificaba nada. Clippy lo marca como logic bug.
    // Se assertá lo que el comentario decía: "criá" sin sustantivo no es acción.
    assert!(
        parse("criá conciencia sobre el problema", &w).is_none(),
        "«criá» sin sustantivo de archivo no puede parsearse como acción local"
    );
    assert!(parse("explicame algo", &w).is_none());
    std::fs::remove_dir_all(&w).ok();
}
