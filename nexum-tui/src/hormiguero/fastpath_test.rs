//! Tests del router determinístico in-process. Cubren paridad con la capa
//! determinística de classifier.py + el gate de latencia del hot path (Fase 4).

use super::*;

fn is_local(v: &FastVerdict) -> bool {
    matches!(v, FastVerdict::LocalAnswer(_))
}

#[test]
fn test_saludo_es_respuesta_local() {
    let v = classify("hola", "es");
    assert!(is_local(&v), "saludo → respuesta local: {v:?}");
    if let FastVerdict::LocalAnswer(a) = v {
        assert!(a.contains("Acá estoy"), "answer smalltalk es: {a}");
    }
}

#[test]
fn test_status_estas_ahi_es_local() {
    // "hola Nexum, estás?" matchea smalltalk (primer patrón), no status:
    // paridad exacta con classifier.py (orden de patrones idéntico).
    let v = classify("hola Nexum, estás?", "es");
    assert!(is_local(&v), "estás → local: {v:?}");
}

#[test]
fn test_status_puro_devuelve_answer_status() {
    let v = classify("¿estás?", "es");
    assert!(is_local(&v));
    if let FastVerdict::LocalAnswer(a) = v {
        assert!(a.contains("activo"), "answer status: {a}");
    }
}

#[test]
fn test_comando_interfaz_es_local() {
    let v = classify("pará", "es");
    assert!(is_local(&v), "comando → local: {v:?}");
    if let FastVerdict::LocalAnswer(a) = v {
        assert!(a.contains("comando de la interfaz"), "answer command: {a}");
    }
}

#[test]
fn test_tarea_compleja_escala() {
    assert_eq!(
        classify("analizá la arquitectura del sistema", "es"),
        FastVerdict::Escalate,
        "hint de complejidad → escalar"
    );
    assert_eq!(
        classify("refactorizá este módulo", "es"),
        FastVerdict::Escalate
    );
    assert_eq!(
        classify("implementá un parser de JSON", "es"),
        FastVerdict::Escalate
    );
}

#[test]
fn test_codigo_escala() {
    assert_eq!(
        classify("def foo(): return 1", "es"),
        FastVerdict::Escalate,
        "código → escalar (hint ```/def)"
    );
}

#[test]
fn test_pregunta_ambigua_escala_fail_safe() {
    // No matchea trivial ni es claramente compleja: fail-safe = escalar.
    // "false local es más grave que false escalate".
    assert_eq!(
        classify("¿cuál es la capital de Francia?", "es"),
        FastVerdict::Escalate
    );
}

#[test]
fn test_texto_largo_escala() {
    let largo = "hola ".repeat(40); // >120 chars, aunque empiece con saludo
    assert_eq!(
        classify(&largo, "es"),
        FastVerdict::Escalate,
        "input largo no es trivial"
    );
}

#[test]
fn test_ingles_smalltalk_local() {
    let v = classify("hi there", "en");
    assert!(is_local(&v));
    if let FastVerdict::LocalAnswer(a) = v {
        assert!(a.contains("listening"), "answer en: {a}");
    }
}

#[test]
fn test_gracias_es_local() {
    assert!(is_local(&classify("gracias", "es")));
    assert!(is_local(&classify("dale", "es")));
}

#[test]
fn stable_hola_is_one_shot_and_advanced_work_is_rejected() {
    assert_eq!(classify_stable("Hola", "es"), StableFastVerdict::OneShot);
    for prompt in [
        "Analizá el repositorio, modificá archivos y ejecutá tests.",
        "Analizá el repositorio y modificá archivos.",
        "Ejecutá todos los tests.",
        "Usá herramientas para corregir el proyecto.",
        "Revisá el código y aplicá cambios.",
        "Abrí archivos y ejecutá comandos.",
    ] {
        assert_eq!(
            classify_stable(prompt, "es"),
            StableFastVerdict::RejectedByPolicy,
            "{prompt:?} debe rechazarse antes del provider"
        );
    }
}

#[test]
fn conversational_text_does_not_take_silent_local_route() {
    assert_eq!(
        classify_stable("gracias", "es"),
        StableFastVerdict::OneShot
    );
    assert_eq!(
        classify_stable("¿estás ahí?", "es"),
        StableFastVerdict::OneShot
    );
}

#[test]
fn explicit_control_can_use_local_route() {
    assert!(matches!(
        classify_stable("pará", "es"),
        StableFastVerdict::LocalAnswer(_)
    ));
}

#[test]
fn normal_generation_prompt_uses_llm() {
    assert_eq!(
        classify_stable(
            "Escribí una historia de aproximadamente 400 palabras sobre una persona que cambia de profesión.",
            "es"
        ),
        StableFastVerdict::OneShot
    );
}

#[test]
fn long_generation_prompt_uses_llm() {
    let prompt = format!(
        "Escribí una historia de 400 palabras sobre una persona. {}",
        "Incluí detalles humanos y un cierre claro. ".repeat(8)
    );
    assert_eq!(
        classify_stable(&prompt, "es"),
        StableFastVerdict::OneShot
    );
}

#[test]
fn test_nunca_panic_en_inputs_raros() {
    // Unicode, vacío, whitespace, emojis, null-ish: jamás panic.
    for t in ["", "   ", "🚀🔥", "\n\t", "ñ", "a".repeat(5000).as_str()] {
        let _ = classify(t, "es");
    }
}

/// GATE Fase 4: el hot path del Hormiguero decide en microsegundos.
/// Sin red, la latencia del thread de UI es sub-milisegundo << 5ms p95.
#[test]
fn test_latencia_hot_path_bajo_gate() {
    let casos = [
        "hola",
        "¿estás?",
        "pará",
        "analizá la arquitectura",
        "¿cuál es la capital de Francia?",
        "gracias por todo",
    ];
    // Warmup (compila las regex vía OnceLock).
    for c in &casos {
        let _ = classify(c, "es");
    }
    let mut lat = Vec::with_capacity(casos.len() * 200);
    for _ in 0..200 {
        for c in &casos {
            let t0 = std::time::Instant::now();
            let _ = classify(c, "es");
            lat.push(t0.elapsed().as_micros() as u64);
        }
    }
    lat.sort_unstable();
    let p95 = lat[(lat.len() as f64 * 0.95) as usize];
    // Gate: p95 < 5ms = 5000µs. En la práctica es de decenas de µs.
    assert!(
        p95 < 5000,
        "hot path p95 = {p95}µs debe ser < 5000µs (gate UI-block Fase 4)"
    );
}
