//! Conformidad del router determinístico contra el corpus versionado
//! (OMEGA Fase 6). corpus_v1.json es la tabla de verdad ÚNICA: también corre
//! contra el classifier Python (tests/test_routing_conformance.py) para el
//! gate RUST_PYTHON_ROUTING_DIVERGENCE = 0.

use serde::Deserialize;

use super::{classify, FastVerdict};

#[derive(Deserialize)]
struct Corpus {
    version: u32,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    text: String,
    expect: String,
    class: String,
    #[serde(default)]
    sensitive: bool,
}

const CORPUS: &str = include_str!("corpus_v1.json");

fn load() -> Corpus {
    serde_json::from_str(CORPUS).expect("corpus_v1.json inválido")
}

#[test]
fn test_corpus_version_y_tamano() {
    let c = load();
    assert_eq!(c.version, 1);
    assert!(c.cases.len() >= 70, "corpus con cobertura real: {}", c.cases.len());
}

#[test]
fn test_corpus_paridad_total() {
    let c = load();
    let mut fallas: Vec<String> = Vec::new();
    for case in &c.cases {
        let got = match classify(&case.text, "es") {
            FastVerdict::LocalAnswer(_) => "local",
            FastVerdict::Escalate => "escalate",
        };
        if got != case.expect {
            fallas.push(format!(
                "[{}] {:?}: esperado {}, obtuvo {}",
                case.class, case.text, case.expect, got
            ));
        }
    }
    assert!(fallas.is_empty(), "divergencias con el corpus:\n{}", fallas.join("\n"));
}

/// Regla dura: un caso sensible (security/adversarial) JAMÁS puede resolverse
/// local. false local sensible = 0.
#[test]
fn test_corpus_sensibles_jamas_local() {
    let c = load();
    for case in c.cases.iter().filter(|c| c.sensitive) {
        assert_eq!(
            classify(&case.text, "es"),
            FastVerdict::Escalate,
            "caso sensible NO puede ser local: {:?}",
            case.text
        );
    }
}

/// GATE Fase 6: p95 del router determinístico < 2 ms sobre el corpus entero.
#[test]
fn test_corpus_latencia_p95_bajo_2ms() {
    let c = load();
    // warmup (regex OnceLock)
    for case in &c.cases {
        let _ = classify(&case.text, "es");
    }
    let mut lat = Vec::with_capacity(c.cases.len() * 20);
    for _ in 0..20 {
        for case in &c.cases {
            let t0 = std::time::Instant::now();
            let _ = classify(&case.text, "es");
            lat.push(t0.elapsed().as_micros() as u64);
        }
    }
    lat.sort_unstable();
    let p95 = lat[(lat.len() as f64 * 0.95) as usize];
    assert!(p95 < 2_000, "router determinístico p95 = {p95}µs debe ser < 2ms");
}
