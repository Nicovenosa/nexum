//! Router determinístico in-process del Hormiguero (OMEGA Fase 4 + base Fase 6).
//!
//! Por qué existe: el pre-route corría en `submit_message` (thread de UI) y
//! hacía una llamada HTTP sync al sidecar Python — hasta ~800ms de bloqueo si
//! el sidecar colgaba (hallazgo H-1). Este módulo decide **in-process, cero
//! red, en microsegundos**, eliminando todo network-wait del hot path.
//!
//! Es un espejo FIEL de la capa determinística de `classifier.py` (pasos 1, 2
//! y 4: triviales → respuesta local; hints de complejidad / largo / fail-safe
//! → escalar). El paso 3 (residual LLM qwen, ya OPT-IN y OFF por defecto) NO
//! vive acá: queda para el camino async del sidecar, fuera del thread de UI.
//!
//! PRIVACIDAD: no loggea el texto del usuario. Nunca.

use std::sync::OnceLock;

use regex::Regex;

/// Veredicto del hot path. Determinístico y total: o responde local, o escala.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FastVerdict {
    /// Trivial de alta confianza resuelto sin provider pago.
    LocalAnswer(String),
    /// No clasifica como trivial confiable → al modelo principal (fail-safe).
    /// "false local es más grave que false escalate": ante la duda, escalar.
    Escalate,
}

/// Veredicto exclusivo del perfil estable. Conserva las respuestas locales
/// seguras, pero separa explícitamente ONE_SHOT de REJECTED_BY_POLICY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StableFastVerdict {
    LocalAnswer(String),
    OneShot,
    RejectedByPolicy,
}

/// Umbral de longitud para intentar la vía trivial (idéntico a classifier.py).
const TRIVIAL_MAX_CHARS: usize = 120;

struct Patterns {
    /// (intent, regex). El orden importa: se evalúa de arriba a abajo.
    trivial: Vec<(&'static str, Regex)>,
    complex_hints: Regex,
    safe_text_generation: Regex,
    operational_hints: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        // `(?i)` = case-insensitive (equivale a re.I del Python). Cada patrón
        // en una sola línea: los raw strings de Rust no hacen line-continuation.
        let build = |s: &str| Regex::new(s).expect("regex del hormiguero inválida");
        Patterns {
            trivial: vec![
                (
                    // Saludo PURO: saludo + cortesía opcional, NUNCA "saludo +
                    // tarea". (Q-1 0.1.2: el .{0,40} viejo tragaba tareas —
                    // "hola, revisá vulnerabilidades" caía como trivial.)
                    "smalltalk",
                    build(r"(?i)^(hola|buenas(\s+(tardes|noches))?|buen\s?d[ií]a|hey|hi|hello|holis|qu[eé]\s+tal|qu[eé]\s+onda)(\s+nexum)?[\s,.!¡?]*((c[oó]mo\s+(va|and[aá]s|est[aá]s|te\s+va|vas|anda|le\s+va)|qu[eé]\s+tal|todo\s+bien|qu[eé]\s+onda|there|how\s+(are\s+you|is\s+it\s+going|are\s+things)|what'?s\s+up)[\s,.!¡?]*)?$"),
                ),
                (
                    "status",
                    build(r"(?i)(est[aá]s(\s*(ah[ií]|activo|vivo|bien|escuchando))?\s*\??$|escuch[aá]s|are you (there|listening|alive|ok)|me\s+(o[ií]s|escuch[aá]s))"),
                ),
                (
                    "status",
                    build(r"(?i)^(nexum[,\s]+)?(est[aá]s|estas)\s*\??\s*$"),
                ),
                (
                    "command",
                    build(r"(?i)^(par[aá]|stop|cancel[aá]?|cancel|repet[ií]|repeat|le[eé]\s+el\s+[uú]ltimo\s+mensaje|copi[aá]\s+la\s+respuesta|mostrame\s+(ayuda|proveedores|modelos)|abr[ií]\s+ayuda|estado\s+del\s+sistema|no\s+hagas\s+nada.{0,30})\s*\.?\s*$"),
                ),
                (
                    // Agradecimiento PURO, no "gracias + tarea" (Q-1 0.1.2).
                    "smalltalk",
                    build(r"(?i)^(gracias(\s+(totales|che|nexum|mil))?|muchas\s+gracias|thanks|thank\s+you|genial|ok(\s+gracias)?|dale|listo|perfecto|buen[ií]simo|joya|de\s+diez|de\s+una)\s*[?!.]*$"),
                ),
            ],
            complex_hints: build(r"(?i)(analiz|modific|ejecut|herramient|correg|revis|aplic[aá]?\s+cambi|abr[ií]\s+archiv|comandos?|refactor|dise[ñn]|arquitect|debug|implement|investig|optimiz|escrib[ií]|gener[aá]|explic[aá]\s+en\s+detalle|compar[aá]|audit|```|def |fn |class |import |trade-?off)"),
            safe_text_generation: build(
                r"(?i)^\s*(escrib[ií]|redact[aá]|gener[aá]|cont[aá]\s+una\s+historia|explic[aá]|resum[ií]|analiz[aá]\s+(un|una|el|la)\s+(concepto|idea|tema|tecnolog[ií]a))",
            ),
            operational_hints: build(
                r"(?i)(repositorio|archivos?|c[oó]digo|tests?|pruebas?|comandos?|terminal|shell|git|modific|ejecut|implement|refactor|debug|herramient|web|internet|busc[aá]|investig)",
            ),
        }
    })
}

/// Respuesta enlatada por intent (espejo de `_LOCAL_ANSWERS_ES/EN`).
fn local_answer(intent: &str, locale: &str) -> Option<String> {
    let es = locale.starts_with("es");
    let ans = match (intent, es) {
        ("smalltalk", true) => "¡Hola! Acá estoy, escuchando. ¿En qué te ayudo?",
        ("status", true) => "Sí, estoy activo. El runtime local de Nexum está funcionando y listo.",
        ("command", true) => "Entendido. Ese es un comando de la interfaz: usá el atajo o comando correspondiente (por ejemplo /help para ayuda, Ctrl+C para copiar la última respuesta).",
        ("smalltalk", false) => "Hi! I'm here and listening. How can I help?",
        ("status", false) => "Yes, I'm up. Nexum's local runtime is running and ready.",
        ("command", false) => "Got it. That's an interface command: use the matching shortcut or slash command (e.g. /help, or Ctrl+C to copy the last answer).",
        _ => return None,
    };
    Some(ans.to_string())
}

/// Clasifica el input en el hot path. **Cero red, sin locks, sin panic.**
/// Espejo de `classifier.classify()` pasos 1/2/4.
pub fn classify(text: &str, locale: &str) -> FastVerdict {
    let stripped = text.trim();
    let n_chars = stripped.chars().count();
    let p = patterns();

    // Paso 1: determinístico trivial (corto y sin señales de complejidad).
    if n_chars <= TRIVIAL_MAX_CHARS && !p.complex_hints.is_match(stripped) {
        for (intent, re) in &p.trivial {
            if re.is_match(stripped) {
                if let Some(ans) = local_answer(intent, locale) {
                    return FastVerdict::LocalAnswer(ans);
                }
            }
        }
    }

    // Pasos 2 y 4: señal de complejidad, largo, o sin match confiable → escalar.
    // (El residual LLM del paso 3 vive fuera del hot path.)
    FastVerdict::Escalate
}

/// Clasificación única para un turno estable.
///
/// Los saludos puros entran en ONE_SHOT para que el camino de demo ejercite el
/// provider acotado. Los comandos/status/agradecimientos conservan respuesta
/// local. La generación puramente textual puede usar la llamada one-shot
/// (sin tools); tareas operativas continúan rechazadas antes del provider.
pub fn classify_stable(text: &str, locale: &str) -> StableFastVerdict {
    let stripped = text.trim();
    let p = patterns();
    let n_chars = stripped.chars().count();

    let safe_text_generation = n_chars <= 4_000
        && p.safe_text_generation.is_match(stripped)
        && !p.operational_hints.is_match(stripped);
    if !safe_text_generation
        && (n_chars > TRIVIAL_MAX_CHARS || p.complex_hints.is_match(stripped))
    {
        return StableFastVerdict::RejectedByPolicy;
    }

    for (intent, re) in &p.trivial {
        if !re.is_match(stripped) {
            continue;
        }
        // El primer patrón smalltalk es el saludo. Los agradecimientos son
        // smalltalk también, pero aparecen después de status/command.
        if *intent == "smalltalk"
            && matches!(
                stripped
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .trim_matches(|c: char| !c.is_alphabetic())
                    .to_ascii_lowercase()
                    .as_str(),
                "hola" | "holis" | "buenas" | "buen" | "hey" | "hi" | "hello" | "qué"
            )
        {
            return StableFastVerdict::OneShot;
        }
        // En el perfil de demo sólo comandos de control explícitos conservan
        // respuesta local. Smalltalk y status conversacionales ejercitan el
        // provider seleccionado y quedan observables en /trace.
        if *intent == "command" {
            let Some(answer) = local_answer(intent, locale) else {
                continue;
            };
            return StableFastVerdict::LocalAnswer(answer);
        }
    }

    StableFastVerdict::OneShot
}

#[cfg(test)]
#[path = "fastpath_test.rs"]
mod fastpath_test;

#[cfg(test)]
#[path = "corpus_test.rs"]
mod corpus_test;
