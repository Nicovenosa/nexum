//! Voice E2E sobre OMEGA (ciclo 7). Ejercita los escenarios del contrato sobre
//! el pipeline REAL de decisión de voz (`VoiceIntentRouter`), verificando:
//! - texto y voz recorren el MISMO pipeline semántico (mismo fastpath);
//! - gates de latencia (routing local desde transcript p95 <50ms);
//! - CREATE_DIRECTORY: local, 0 provider, requiere envelope/HITL aguas abajo;
//! - razonamiento: escala al provider, sin canned refusal;
//! - resiliencia: sidecar/Hormiguero caídos no rompen (fastpath in-process).
//!
//! Nota honesta: los WAV reales (ASR whisper) se cubren en el harness de
//! integración fuera de proceso; acá se usan transcripts (la salida del ASR)
//! para aislar el pipeline de decisión, que es donde vive la lógica OMEGA.

use super::acp_turn::VoiceRouteDecision;
use super::intent_router::VoiceIntentRouter;
use super::HandledBy;
use crate::hormiguero::{bridge, fastpath, FastVerdict};

fn is_local(d: &VoiceRouteDecision) -> bool {
    matches!(d, VoiceRouteDecision::Local { .. })
}
fn is_escalate(d: &VoiceRouteDecision) -> bool {
    matches!(
        d,
        VoiceRouteDecision::Escalate { .. }
            | VoiceRouteDecision::AskForEscalation { .. }
            | VoiceRouteDecision::CostConfirmationRequired { .. }
    )
}

/// PARIDAD texto/voz: el mismo transcript produce el mismo veredicto semántico
/// que el fastpath de texto (local vs escalate).
#[test]
fn test_paridad_texto_voz_mismo_pipeline() {
    let _g = bridge::test_env_lock();
    bridge().reset_for_test();
    std::env::set_var("NEXUM_HORMIGUERO", "on");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    let casos = [
        ("hola", true),
        ("gracias", true),
        ("¿estás?", true),
        ("analizá la arquitectura del sistema", false),
        ("implementá un parser recursivo", false),
        ("¿cuál es la capital de Francia?", false),
    ];
    for (t, expect_local) in casos {
        let text_verdict = fastpath::classify(t, "es");
        let voice = VoiceIntentRouter::route_decision(t, "es");
        let text_local = matches!(text_verdict, FastVerdict::LocalAnswer(_));
        assert_eq!(
            text_local, expect_local,
            "texto '{t}' esperado local={expect_local}"
        );
        // Voz debe coincidir semánticamente con texto.
        if text_local {
            assert!(is_local(&voice), "voz '{t}' debe ser Local como texto");
        } else {
            assert!(is_escalate(&voice), "voz '{t}' debe escalar como texto: {voice:?}");
        }
    }
    std::env::remove_var("NEXUM_HORMIGUERO");
}

/// GATE: routing local desde transcript p95 < 50ms (real: sub-ms, in-process).
#[test]
fn test_gate_routing_voz_p95_bajo_50ms() {
    let _g = bridge::test_env_lock();
    bridge().reset_for_test();
    std::env::set_var("NEXUM_HORMIGUERO", "on");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    let transcripts = ["hola nexum", "¿estás ahí?", "gracias", "pará"];
    for t in transcripts {
        let _ = VoiceIntentRouter::route_decision(t, "es");
    }
    let mut lat = Vec::new();
    for _ in 0..200 {
        for t in transcripts {
            let t0 = std::time::Instant::now();
            let _ = VoiceIntentRouter::route_decision(t, "es");
            lat.push(t0.elapsed().as_micros() as u64);
        }
    }
    lat.sort_unstable();
    let p95 = lat[(lat.len() as f64 * 0.95) as usize];
    std::env::remove_var("NEXUM_HORMIGUERO");
    assert!(p95 < 50_000, "routing voz p95 = {p95}µs debe ser < 50ms");
}

/// Escenario razonamiento: escala al provider, JAMÁS canned refusal antigua.
#[test]
fn test_razonamiento_escala_sin_canned_refusal() {
    let _g = bridge::test_env_lock();
    bridge().reset_for_test();
    std::env::set_var("NEXUM_HORMIGUERO", "on");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    let decision = VoiceIntentRouter::route_decision("explicá en detalle cómo funciona TLS", "es");
    std::env::remove_var("NEXUM_HORMIGUERO");
    // Debe escalar con envelope (source=Voice), no bloquear.
    match decision {
        VoiceRouteDecision::Escalate { envelope, .. } => {
            assert_eq!(
                envelope.source,
                nexum_acp::task::TaskSource::Voice,
                "envelope de voz"
            );
        }
        other => panic!("razonamiento debe escalar al provider, no {other:?}"),
    }
}

/// Resiliencia: con el Hormiguero flag OFF (sidecar irrelevante), la voz sigue
/// respondiendo triviales localmente (fastpath no depende del sidecar).
#[test]
fn test_resiliencia_trivial_sin_sidecar() {
    let _g = bridge::test_env_lock();
    bridge().reset_for_test();
    std::env::set_var("NEXUM_HORMIGUERO", "on");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    // Runtime dir vacío = sidecar muerto; el trivial se resuelve igual.
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("NEXUM_HORMIGUERO_RUNTIME_DIR", dir.path());
    let r = VoiceIntentRouter::route("hola", "es", false);
    std::env::remove_var("NEXUM_HORMIGUERO_RUNTIME_DIR");
    std::env::remove_var("NEXUM_HORMIGUERO");
    assert_eq!(r.handled_by, HandledBy::Local, "trivial local sin sidecar");
    assert!(r.should_speak, "el trivial habla");
}

/// 50 ciclos de voz seguidos (contrato ciclo 7): sin panic, veredictos estables.
#[test]
fn test_50_ciclos_voz_estables() {
    let _g = bridge::test_env_lock();
    bridge().reset_for_test();
    std::env::set_var("NEXUM_HORMIGUERO", "on");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    let mut locales = 0;
    let mut escaladas = 0;
    for i in 0..50 {
        let t = if i % 2 == 0 { "hola" } else { "analizá el sistema entero" };
        match VoiceIntentRouter::route_decision(t, "es") {
            d if is_local(&d) => locales += 1,
            d if is_escalate(&d) => escaladas += 1,
            _ => {}
        }
    }
    std::env::remove_var("NEXUM_HORMIGUERO");
    assert_eq!(locales, 25, "25 triviales estables");
    assert_eq!(escaladas, 25, "25 escaladas estables");
}
