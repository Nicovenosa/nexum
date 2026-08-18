//! Clasificador de flujo determinístico para el LOCAL_FAST path.
//!
//! `NEXUM_LOCAL_FAST_DEMO_V1` — alcance: SÓLO `DIRECT_CHAT` vs el resto.
//!
//! Una decisión previa al turno, **sin LLM**, que detecta consultas simples de
//! conversación ("Respondé únicamente X", saludos, preguntas triviales) para
//! rutearlas a un camino de contexto mínimo y sin tools. Todo lo que no sea un
//! `DIRECT_CHAT` claro cae —de forma explícita y conservadora— a `FullReact`,
//! que preserva el comportamiento completo actual (fallback seguro).
//!
//! El flujo aguas arriba (TaskEnvelopeV1 → Hormiguero → ACP) NO se altera: la
//! clasificación sólo decide qué system prompt y qué tools se construyen dentro
//! del executor. Default **ON**; `NEXUM_LOCAL_FAST=0` lo apaga (ver
//! [`local_fast_enabled`]).

/// Resultado de la clasificación de flujo. En esta versión sólo se distingue
/// el camino rápido de conversación directa del resto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowClass {
    /// Consulta simple sin tools: contexto mínimo, un solo roundtrip.
    DirectChat,
    /// Todo lo demás: preserva el flujo ReAct completo (comportamiento actual).
    FullReact,
}

/// Longitud máxima (en caracteres) de un prompt que puede considerarse
/// `DIRECT_CHAT`. Por encima, escala a `FullReact` (probable tarea con
/// contexto). Conservador a propósito.
/// Prefijos con los que el usuario FUERZA el camino con tools. El automatismo
/// acierta la mayoría de las veces; cuando no, manda el usuario.
pub const FORCE_TOOLS_PREFIXES: &[&str] = &["!", "/tools ", "/agente ", "/agent "];

/// Longitud a partir de la cual un mensaje se mira con más cuidado. NO es un
/// criterio de escalado por sí solo: un texto largo sin ninguna señal de
/// intención sigue siendo charla. Sólo se usa para el corte de multi-paso.
const LONG_MESSAGE_CHARS: usize = 240;

/// Señales que FUERZAN `FullReact` aunque el prompt sea corto: indican que se
/// necesita una tool, un archivo, código o una tarea de agente. La lista es
/// deliberadamente amplia (preferimos escalar de más que de menos).
/// Señales que se buscan como PALABRA COMPLETA (o prefijo de palabra).
///
/// Antes se buscaban por substring y eso escalaba charlas: "implementación"
/// matcheaba "implementa", "buscador" matcheaba "busca", "testigo" matcheaba
/// "test". Justo el falso positivo que esta fase venía a eliminar.
///
/// Se usa prefijo de palabra —no igualdad— para cubrir la conjugación del
/// español sin listar cada forma: "leé/leer/leelo" arrancan con "le".
///
/// # Regla para agregar señales nuevas
///
/// **Un verbo cuyo prefijo colisiona con palabras cotidianas no va en esta
/// tabla.** No es una excepción para casos puntuales: es el criterio.
///
/// El motivo es que el costo y el beneficio son asimétricos. Un pedido real
/// ("listá los archivos de src/", "mostrame el config.toml") casi siempre trae
/// un path o una extensión, y **escala igual por `FULL_REACT_FRAGMENTS`**. En
/// cambio, un stem promiscuo escala charlas: `"list"` matcheaba "listo",
/// "lista" y "listado" — y con él, un simple "LISTO" al final de una frase se
/// iba al loop con tools. Se ganó poco y se rompió el caso común.
///
/// Al evaluar un stem nuevo, preguntarse: ¿qué palabras cotidianas empiezan
/// igual? Si la respuesta no es "ninguna", dejalo afuera. Si el pedido real que
/// querías cubrir no trae path ni extensión, el usuario todavía tiene el
/// prefijo `!` para forzar el camino con tools — que es más barato que una
/// tabla que se dispara sola.
///
/// El margen de +2 caracteres implementa esta regla de forma mecánica: cubre la
/// conjugación y deja afuera los sustantivos derivados.
const FULL_REACT_VERB_STEMS: &[&str] = &[
    // acción sobre el entorno (español rioplatense + neutro)
    "analiz", "busca", "busq", "buscá", "ejecut", "corr", "leé", "leer", "leelo",
    "abrí", "abrir", "abre", "escrib", "cre", "modific", "edit", "borr",
    "elimin", "instal", "descarg", "revis", "arregl", "implement", "refactor",
    "commit", "git", "push", "deploy", "compil", "clon", "mové", "mover",
    "renombr", "mostrame", "mostra",
    // inglés
    "run", "search", "fetch", "write", "create", "delete", "install",
    "download", "review", "fix", "analyze", "build", "test",
];

/// Fragmentos que NO son palabras: paths, extensiones, protocolos. Estos SÍ se
/// buscan por substring porque su sola presencia ya delata intención.
const FULL_REACT_FRAGMENTS: &[&str] = &[
    "http://", "https://", "/home/", "./", "../", ".rs", ".py", ".md",
    ".json", ".toml", ".sh", ".log", ".yaml", ".yml", "mcp", "anytype",
];

/// ¿Alguna palabra del texto empieza con alguno de los stems?
fn has_verb_signal(norm: &str) -> bool {
    norm.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .any(|word| {
            FULL_REACT_VERB_STEMS.iter().any(|stem| {
                // Prefijo de palabra con margen ACOTADO: cubre la conjugación
                // ("busca/buscá/buscar") sin tragarse los sustantivos
                // derivados ("buscador", "implementación", "instalación").
                // El margen es 2: con 3, "buscador" (8) entraba por "busca" (5).
                word.starts_with(stem) && word.len() <= stem.len() + 2
            })
        })
}


/// Prefijos que marcan una consulta de "respondé sólo esto" — el caso canónico
/// de `DIRECT_CHAT`. Normalizados a minúsculas sin tildes por el llamador.
const RESPOND_ONLY_MARKERS: &[&str] = &[
    "responde unicamente",
    "responde solo",
    "responde solamente",
    "respond only",
    "reply only",
    "answer only",
    "answer with only",
    "output only",
];

/// Normaliza para comparación: minúsculas + quita tildes comunes del español.
fn normalize(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' => 'u',
            'ü' => 'u',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

/// Clasifica un `user_input` de forma determinística.
///
/// Escala por INTENCIÓN, no por longitud.
///
/// El default es la charla: si nada indica que hacen falta tools, va a
/// `DirectChat`. Antes era al revés —cualquier cosa que no entrara en 240
/// caracteres se escalaba— y eso mandaba "hola nexum" al loop de ReAct.
///
/// **La duda cae del lado barato.** Si no está claro, `DirectChat`: el peor
/// caso es que el modelo conteste "necesito acceder a un archivo, pedímelo
/// así" y eso cuesta dos segundos. El peor caso del otro lado es un loop de
/// diez iteraciones contra un modelo que no sabe emitir tool calls.
///
/// Reglas, en orden:
/// 1. Prefijo de escalado explícito (`!`, `/tools`) → `FullReact`.
/// 2. Vacío → `DirectChat` (no hay nada que resolver con tools).
/// 3. Señal de intención (tool/archivo/path/código) → `FullReact`.
/// 4. Bloque de código o mensaje largo multi-paso → `FullReact`.
/// 5. Cualquier otro caso → `DirectChat`.
pub fn classify(user_input: &str) -> FlowClass {
    let trimmed = user_input.trim();

    // (1) escalado explícito del usuario: manda sobre el automatismo.
    if FORCE_TOOLS_PREFIXES
        .iter()
        .any(|p| trimmed.starts_with(p))
    {
        return FlowClass::FullReact;
    }

    // (2) vacío: no hay intención que detectar, y menos aún tools que correr.
    if trimmed.is_empty() {
        return FlowClass::DirectChat;
    }

    let norm = normalize(trimmed);

    // (3) señales de intención: verbos por palabra, fragmentos por substring
    if has_verb_signal(&norm) || FULL_REACT_FRAGMENTS.iter().any(|f| norm.contains(f)) {
        return FlowClass::FullReact;
    }

    // (2b) path absoluto genérico: cualquier token que empiece con '/'
    // (ej. "/etc/hostname"). Cubre paths no listados explícitamente.
    if trimmed
        .split_whitespace()
        .any(|tok| tok.starts_with('/') && tok.len() > 1)
    {
        return FlowClass::FullReact;
    }

    // (3) bloque de código explícito o multi-línea sustancial
    if trimmed.contains("```") {
        return FlowClass::FullReact;
    }
    // (4) multi-paso: varias líneas Y mensaje largo. Ninguna de las dos sola
    // alcanza — una pregunta larga sin señales sigue siendo una pregunta.
    let non_empty_lines = trimmed.lines().filter(|l| !l.trim().is_empty()).count();
    if non_empty_lines > 2 && trimmed.chars().count() > LONG_MESSAGE_CHARS {
        return FlowClass::FullReact;
    }

    // El marcador "respondé únicamente …" ya no hace falta para llegar a
    // DirectChat (es el default), pero se conserva porque documenta la
    // intención y protege el caso de los E2E.
    let _ = &RESPOND_ONLY_MARKERS;

    // (5) default: charla. Sin señal de intención, no hay motivo para pagar
    // el costo del loop con tools.
    FlowClass::DirectChat
}

/// Decisión de ruteo `DIRECT_CHAT`, determinística y unit-testeable. Combina:
/// el flag LOCAL_FAST activo, la AUSENCIA de un envelope explícito (no pisamos
/// la semántica de un `TaskEnvelopeV1`) y que el texto clasifique como
/// `DirectChat`. Cualquier condición falsa ⇒ se preserva el flujo completo.
pub fn should_direct_chat(
    local_fast_enabled: bool,
    has_explicit_envelope: bool,
    user_input: &str,
) -> bool {
    local_fast_enabled
        && !has_explicit_envelope
        && matches!(classify(user_input), FlowClass::DirectChat)
}

/// ¿El ruteo LOCAL_FAST está habilitado? **Default ON.**
///
/// Nació como opt-in para no cambiar el comportamiento global sin activación
/// explícita. Ese resguardo dejó de tener sentido cuando el clasificador pasó a
/// escalar por intención: sin el flag, un "hola" paga el loop completo con
/// herramientas, que es exactamente el cuelgue que veníamos arreglando. El
/// comportamiento sano no puede depender de que alguien se acuerde de exportar
/// una variable.
///
/// La variable queda como **interruptor para apagar**, no para prender:
///
/// ```text
/// NEXUM_LOCAL_FAST=0      # vuelve al flujo completo siempre
/// ```
///
/// Valores reconocidos como apagado: `0`, `false`, `off`, `no`. Cualquier otra
/// cosa —incluida la variable ausente— deja el ruteo encendido.
pub fn local_fast_enabled() -> bool {
    local_fast_enabled_from(std::env::var("NEXUM_LOCAL_FAST").ok().as_deref())
}

/// Núcleo puro de [`local_fast_enabled`], separado del entorno para poder
/// testear la polaridad sin mutar variables globales durante los tests.
pub fn local_fast_enabled_from(valor: Option<&str>) -> bool {
    !matches!(
        valor.map(str::trim),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

#[cfg(test)]
#[path = "flow_test.rs"]
mod flow_test;
