use super::super::http_testutil::mock_route_server;
use super::*;
use std::time::Duration;

/// Prepara env para que discover() encuentre el "sidecar" mock: escribe
/// port/token en un dir temporal y apunta NEXUM_HORMIGUERO_RUNTIME_DIR ahí.
fn setup_env(port: u16, token: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("hormiguero.port"), port.to_string()).unwrap();
    std::fs::write(dir.path().join("hormiguero.token"), token).unwrap();
    std::env::set_var("NEXUM_HORMIGUERO_RUNTIME_DIR", dir.path());
    std::env::set_var("NEXUM_HORMIGUERO", "on");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    dir
}

fn teardown_env() {
    std::env::remove_var("NEXUM_HORMIGUERO_RUNTIME_DIR");
    std::env::remove_var("NEXUM_HORMIGUERO");
}

const HEALTH_OK_BODY: &str = r#"{"ok":true,"service":"hormiguero","version":"1","uptime_ms":10}"#;

#[test]
fn test_flag_off_es_passthrough() {
    let _guard = test_env_lock();
    teardown_env();
    bridge().reset_for_test();
    // Sin flag: passthrough inmediato, sin clasificar.
    assert_eq!(bridge().route_text("hola", "es"), RouteOutcome::Passthrough);
}

#[test]
fn test_trivial_respuesta_local_in_process() {
    let _guard = test_env_lock();
    bridge().reset_for_test();
    std::env::set_var("NEXUM_HORMIGUERO", "on");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    // Sin sidecar publicado: el hot path decide igual (in-process, cero red).
    let out = bridge().route_text("hola", "es");
    teardown_env();
    assert!(
        matches!(out, RouteOutcome::LocalAnswer(ref a) if a.contains("Acá estoy")),
        "trivial → respuesta local in-process, sin sidecar: {out:?}"
    );
}

#[test]
fn test_complejo_escala_needs_paid_ai() {
    let _guard = test_env_lock();
    bridge().reset_for_test();
    std::env::set_var("NEXUM_HORMIGUERO", "on");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    let out = bridge().route_text("analizá la arquitectura del sistema", "es");
    teardown_env();
    assert_eq!(
        out,
        RouteOutcome::NeedsPaidAi(None),
        "complejo → escalar (sin envelope en el hot path): {out:?}"
    );
}

#[test]
fn stable_routes_always_create_an_envelope_with_the_original_decision() {
    let _guard = test_env_lock();
    bridge().reset_for_test();
    let simple = bridge().route_stable_text("Hola", "es");
    let advanced = bridge().route_stable_text(
        "Analizá el repositorio, modificá archivos y ejecutá tests.",
        "es",
    );
    assert!(matches!(
        simple,
        RouteOutcome::NeedsPaidAi(Some(envelope))
            if envelope.route_decision == StableRouteDecision::OneShot
                && envelope.task_classification == StableTaskClassification::Simple
    ));
    assert!(matches!(
        advanced,
        RouteOutcome::NeedsPaidAi(Some(envelope))
            if envelope.route_decision == StableRouteDecision::RejectedByPolicy
                && envelope.task_classification == StableTaskClassification::Advanced
    ));
    let counters = bridge().status().counters;
    assert_eq!(counters.stable_one_shot, 1);
    assert_eq!(counters.stable_rejected, 1);
    assert_eq!(counters.escalations, 0);
}

/// GATE Fase 4 (cierre H-1): el hot path NO toca la red. Aunque el sidecar
/// publicado sea un servidor colgado 3s, route_text vuelve en microsegundos.
#[test]
fn test_hot_path_no_toca_red_ni_con_sidecar_colgado() {
    let _guard = test_env_lock();
    bridge().reset_for_test();
    // "sidecar" que tardaría 3s en responder — si el hot path lo llamara,
    // bloquearía la UI. Publicamos su port/token para tentar a discover().
    let port = mock_route_server(200, HEALTH_OK_BODY, 3000);
    let _dir = setup_env(port, "tok123");
    let start = std::time::Instant::now();
    let out = bridge().route_text("hola", "es");
    let elapsed = start.elapsed();
    teardown_env();
    assert!(
        matches!(out, RouteOutcome::LocalAnswer(_)),
        "decide local in-process: {out:?}"
    );
    assert!(
        elapsed < Duration::from_millis(50),
        "hot path NO debe tocar el sidecar colgado: tardó {elapsed:?}"
    );
}

#[test]
fn test_contadores_local_y_escalada() {
    let _guard = test_env_lock();
    bridge().reset_for_test();
    std::env::set_var("NEXUM_HORMIGUERO", "on");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    let _ = bridge().route_text("hola", "es"); // local
    let _ = bridge().route_text("implementá un compilador", "es"); // escala
    let _ = bridge().route_text("gracias", "es"); // local
                                                  // status() con flag off no toca red: solo lee contadores.
    std::env::remove_var("NEXUM_HORMIGUERO");
    let c = bridge().status().counters;
    teardown_env();
    assert_eq!(c.local_answers, 2, "dos respuestas locales");
    assert_eq!(c.escalations, 1, "una escalada");
}

/// El breaker ahora lo alimenta el probe de salud de status() (fuera del hot
/// path). Sidecar muerto (connect refused) → tras 3 fallos el breaker abre.
/// TTL=0 desactiva el cache negativo para forzar un probe por llamada.
#[test]
fn test_status_breaker_abre_con_sidecar_muerto() {
    let _guard = test_env_lock();
    bridge().reset_for_test();
    std::env::set_var("NEXUM_HORMIGUERO_STATUS_TTL_MS", "0");
    // Puerto que nadie escucha (connect refused) con archivos publicados.
    let dead_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let _dir = setup_env(dead_port, "tok123");
    let mut last = None;
    for _ in 0..3 {
        last = Some(bridge().status());
    }
    teardown_env();
    std::env::remove_var("NEXUM_HORMIGUERO_STATUS_TTL_MS");
    let st = last.unwrap();
    assert_eq!(
        st.breaker,
        BreakerState::Open,
        "3 probes fallidos → breaker OPEN"
    );
    assert!(st.consecutive_failures >= 3);
    assert!(!st.sidecar_alive, "sidecar muerto no está alive");
}

/// Mock persistente que cuenta conexiones (a diferencia de mock_route_server,
/// que sirve UNA sola). Respuesta válida para /health y /status a la vez.
fn counting_server(counter: std::sync::Arc<std::sync::atomic::AtomicU32>) -> u16 {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let body = r#"{"ok":true,"service":"hormiguero-sidecar","version":"1","uptime_ms":1,"hormiguero_available":true,"model_available":true,"model":"nexum-local","mode":"safe","capabilities":[]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    port
}

/// GATE Fase 9 (cierra H-3): 10.000 status() con cache TTL ⇒ probes acotados
/// (2 conexiones: health+status del primer probe), p95 <10ms, cero inferencia.
#[test]
fn test_status_cache_10k_llamadas_probes_acotados() {
    let _guard = test_env_lock();
    bridge().reset_for_test();
    std::env::remove_var("NEXUM_HORMIGUERO_STATUS_TTL_MS"); // TTL default 5s
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let port = counting_server(counter.clone());
    let _dir = setup_env(port, "tok123");
    let mut lat = Vec::with_capacity(10_000);
    for _ in 0..10_000 {
        let t0 = std::time::Instant::now();
        let st = bridge().status();
        lat.push(t0.elapsed().as_micros() as u64);
        assert!(
            st.sidecar_alive,
            "cache sirve el resultado del primer probe"
        );
    }
    teardown_env();
    let conns = counter.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        conns <= 4,
        "10k status ⇒ probes acotados por TTL (esperado 2 conexiones, hubo {conns})"
    );
    lat.sort_unstable();
    let p95 = lat[(lat.len() as f64 * 0.95) as usize];
    assert!(
        p95 < 10_000,
        "status p95 = {p95}µs debe ser < 10ms con cache"
    );
}

#[test]
fn test_status_flag_off_no_toca_red() {
    let _guard = test_env_lock();
    bridge().reset_for_test();
    teardown_env(); // flag off
    let st = bridge().status();
    assert!(!st.enabled);
    assert!(!st.sidecar_alive);
}
