//! Log JSONL del camino de chat (3.1).
//!
//! Los archivos de `~/.nexum/metrics/` estaban en **0 bytes desde el 2 de
//! julio**: el directorio existía, Doctor daba PASS sobre su presencia, y no
//! había una sola línea de código que escribiera en él. Cuando la TUI se colgó
//! un minuto y medio, no quedó rastro de qué flujo se eligió ni de qué hizo el
//! loop.
//!
//! Esto lo arregla para el camino de chat: una línea JSON por evento, append,
//! sin bloquear el turno. Si escribir falla, se pierde el registro pero **nunca
//! se rompe la conversación** — un log que tira el chat abajo es peor que no
//! tener log.
//!
//! Cero secretos: acá van decisiones de ruteo y tiempos, nunca el contenido del
//! prompt, ni credenciales, ni la respuesta del modelo.

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// FUENTE ÚNICA del directorio de métricas, para los dos sinks.
///
/// `turn_log` y `metrics::emit` escriben al MISMO archivo y resolvían la ruta
/// por separado: este respetaba `NEXUM_METRICS_DIR` y el otro iba directo a
/// `nexum_home()/metrics`. Dos lugares para la misma verdad, otra vez.
///
/// # Aislamiento fail-closed bajo test
///
/// El 2026-07-31 una corrida de `cargo test --workspace` escribió eventos en el
/// directorio real del usuario y contaminó la evidencia de un experimento —
/// justamente el experimento que investigaba por qué la traza no servía. Los
/// eventos de fixture se leyeron como uso real y produjeron tres diagnósticos
/// falsos.
///
/// Por eso el aislamiento NO depende de que cada test se acuerde de optar: si
/// el ejecutable es un binario de test, se escribe en un temporal aunque nadie
/// haya seteado nada. Un test que se olvida no puede ensuciar los datos de
/// quien lo corre.
///
/// `cfg!(test)` no alcanza: cuando `nexum-tui` corre sus tests, este crate se
/// compila como dependencia normal y su `cfg(test)` está apagado. La detección
/// tiene que ser en runtime, igual que con `NEXUM_CATALOG_ISOLATED`.
pub fn metrics_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("NEXUM_METRICS_DIR") {
        return Some(PathBuf::from(dir));
    }
    if crate::sandbox::running_under_test() {
        return Some(crate::sandbox::temp_dir_for("metrics"));
    }
    let home = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())?;
    Some(PathBuf::from(home).join(".nexum/metrics"))
}


fn today_file() -> Option<PathBuf> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    // Fecha UTC sin dependencias: días desde epoch → civil (algoritmo de Howard
    // Hinnant), que es lo mismo que usa el resto del stack para nombrar el día.
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some(metrics_dir()?.join(format!("{y:04}-{m:02}-{d:02}.jsonl")))
}

/// Escribe una línea JSON. Best-effort: nunca propaga el error.
fn append(line: String) {
    let Some(path) = today_file() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Ruteo del turno: qué flujo se eligió y con qué contexto. Es el evento que
/// habría contestado "¿en qué modo estaba?" sin tener que deducirlo.
pub fn log_turn_routed(flow: &str, local_fast: bool, explicit_envelope: bool, tools: usize) {
    // usize::MAX es el centinela de "sin límite fijado" (FULL_REACT): sale como
    // -1 para que el JSON sea legible en vez de un número absurdo.
    let tools_field = if tools == usize::MAX {
        "-1".to_string()
    } else {
        tools.to_string()
    };
    append(crate::jsonl::envelope(
        "turn_routed",
        None,
        None,
        &format!(
            r#"{{"flow":"{}","local_fast":{},"explicit_envelope":{},"tools_allowed":{}}}"#,
            crate::jsonl::escape(flow),
            local_fast,
            explicit_envelope,
            tools_field
        ),
    ));
}

/// Qué memoria se inyectó al prompt en este turno.
///
/// Sin esta traza, cuando una respuesta salga rara no hay cómo separar al
/// modelo de lo que la memoria le metió adelante. **Van ids y conteos, nunca el
/// contenido de las memorias**: este archivo no lleva secretos ni datos del
/// usuario.
pub fn log_memory_inject(
    candidatas: usize,
    inyectadas: &[String],
    tokens: u32,
    fuera_umbral: usize,
    fuera_presupuesto: usize,
    sin_ranking: bool,
    ms: u128,
) {
    let ids = inyectadas
        .iter()
        .map(|i| format!(r#""{}""#, crate::jsonl::escape(i)))
        .collect::<Vec<_>>()
        .join(",");
    append(crate::jsonl::envelope(
        "memory_inject",
        None,
        None,
        &format!(
            r#"{{"candidatas":{candidatas},"inyectadas":{},"tokens":{tokens},"fuera_umbral":{fuera_umbral},"fuera_presupuesto":{fuera_presupuesto},"sin_ranking":{sin_ranking},"ms":{ms},"ids":[{ids}]}}"#,
            inyectadas.len()
        ),
    ));
}

/// El turno siguió SIN memoria porque el gateway no respondió.
///
/// Se traza aparte a propósito: "no había nada relevante" y "el sidecar estaba
/// caído" producen la misma respuesta vista desde afuera, y hay que poder
/// distinguirlas al diagnosticar.
pub fn log_memory_degradado(motivo: &str, ms: u128) {
    append(crate::jsonl::envelope(
        "memory_inject",
        None,
        None,
        &format!(
            r#"{{"degradado":true,"motivo":"{}","ms":{ms}}}"#,
            crate::jsonl::escape(motivo)
        ),
    ));
}

/// Provider y modelo con los que se resolvió el turno.
pub fn log_turn_provider(provider: &str, model: &str) {
    append(crate::jsonl::envelope(
        "turn_provider",
        None,
        None,
        &format!(
            r#"{{"provider":"{}","model":"{}"}}"#,
            crate::jsonl::escape(provider),
            crate::jsonl::escape(model)
        ),
    ));
}

/// Una iteración del loop de ReAct. Sin esto, un loop que no converge es
/// indistinguible de un cuelgue.
///
/// `parseable` es el campo que confirma o descarta la hipótesis 4.1: si el
/// modelo nunca emite un tool call interpretable, se ve acá vuelta tras vuelta
/// y el problema es el modelo, no el loop.
pub fn log_react_step(step: usize, max: usize, tool: &str, parseable: bool, elapsed_ms: u128) {
    append(crate::jsonl::envelope(
        "react_step",
        None,
        None,
        &format!(
            r#"{{"step":{},"max":{},"tool":"{}","parseable":{},"elapsed_ms":{}}}"#,
            step, max, crate::jsonl::escape(tool), parseable, elapsed_ms
        ),
    ));
}

/// Lo que sale HACIA EL PROVEEDOR en este turno.
///
/// Cada campo está acá porque contesta una pregunta concreta. Es la lección del
/// turno en que esta traza existía y no pudo contestar si el request duplicaba:
/// la instrumentación se especifica por las preguntas que tiene que responder.
///
///   `messages`   ¿la lista crece linealmente o se duplica?
///   `duplicates` ¿hay mensajes repetidos DENTRO del mismo request? Es el
///                síntoma exacto de re-emitir historial en vez del delta.
///   `chars`      tamaño real del payload, sin depender del tokenizador.
///   `sha`        ¿son los mismos mensajes otra vez, o mensajes distintos?
///
/// Sin umbral y sin condicional. Es el error opuesto al del aviso de cache: allá
/// un threshold disparaba sin datos; acá los datos no pueden faltar por culpa de
/// un threshold. `threshold.token_spike` sólo emitía con output > 4000 y por eso
/// un turno normal no dejaba rastro de consumo.
pub fn log_turn_request(messages: usize, duplicates: usize, chars: usize, sha: &str) {
    append(crate::jsonl::envelope(
        "turn_request",
        None,
        None,
        &format!(
            r#"{{"history_messages":{},"duplicates":{},"chars":{},"sha":"{}"}}"#,
            messages, duplicates, chars, crate::jsonl::escape(sha)
        ),
    ));
}

/// Cómo terminó el turno y cuánto tardó. `reason` distingue una respuesta final
/// de un tope de iteraciones agotado o un error.
pub fn log_turn_end(flow: &str, reason: &str, elapsed_ms: u128) {
    append(crate::jsonl::envelope(
        "turn_end",
        None,
        None,
        &format!(
            r#"{{"flow":"{}","reason":"{}","elapsed_ms":{}}}"#,
            crate::jsonl::escape(flow),
            crate::jsonl::escape(reason),
            elapsed_ms
        ),
    ));
}

#[cfg(test)]
mod tests {
    /// La regresión que este mecanismo cierra: el 2026-07-31 `cargo test
    /// --workspace` escribió en `~/.nexum/metrics/` del usuario y contaminó la
    /// evidencia de un experimento. Los eventos de fixture se leyeron como uso
    /// real y produjeron tres diagnósticos falsos.
    ///
    /// El test corre DENTRO de un binario de test, así que si el aislamiento
    /// funciona no puede resolver la ruta real.
    #[test]
    fn bajo_test_nunca_se_escribe_en_el_directorio_del_usuario() {
        let previo = std::env::var("NEXUM_METRICS_DIR").ok();
        std::env::remove_var("NEXUM_METRICS_DIR");
        let dir = super::metrics_dir().expect("debe resolver algo");
        if let Some(v) = previo {
            std::env::set_var("NEXUM_METRICS_DIR", v);
        }
        if let Ok(home) = std::env::var("HOME") {
            let real = std::path::PathBuf::from(home).join(".nexum/metrics");
            assert_ne!(
                dir, real,
                "sin NEXUM_METRICS_DIR seteado, un binario de test resolvió el \
                 directorio REAL del usuario: la suite volvería a contaminar la evidencia"
            );
        }
        assert!(
            dir.to_string_lossy().contains("nexum-metrics-test-"),
            "esperaba un temporal por PID, salió: {}",
            dir.display()
        );
    }

    /// `NEXUM_METRICS_DIR` explícito sigue mandando: es lo que usan los tests
    /// que necesitan leer lo que escribieron.
    #[test]
    fn la_variable_explicita_tiene_prioridad_sobre_el_aislamiento() {
        let previo = std::env::var("NEXUM_METRICS_DIR").ok();
        std::env::set_var("NEXUM_METRICS_DIR", "/tmp/nexum-explicito");
        let dir = super::metrics_dir().unwrap();
        match previo {
            Some(v) => std::env::set_var("NEXUM_METRICS_DIR", v),
            None => std::env::remove_var("NEXUM_METRICS_DIR"),
        }
        assert_eq!(dir, std::path::PathBuf::from("/tmp/nexum-explicito"));
    }

    use super::*;

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn con_dir_temporal_named(nombre: &str, f: impl FnOnce(&PathBuf)) {
        // NEXUM_METRICS_DIR es estado global del proceso: los tests se
        // serializan o se pisan entre sí.
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // ALLOW justificado: ya es el patrón correcto —PID más nombre del
        // caso— y va dentro del propio módulo que define el aislamiento, así
        // que no puede colgar de sí mismo sin recursión.
        #[allow(clippy::disallowed_methods)]
        let dir = std::env::temp_dir()
            .join(format!("nexum-turnlog-{}-{}", std::process::id(), nombre));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("NEXUM_METRICS_DIR", &dir);
        f(&dir);
        std::env::remove_var("NEXUM_METRICS_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn leer(dir: &PathBuf) -> String {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
            .collect()
    }

    #[test]
    fn el_ruteo_deja_una_linea_legible() {
        con_dir_temporal_named("ruteo", |dir| {
            log_turn_routed("DIRECT_CHAT", true, false, 0);
            let c = leer(dir);
            assert!(c.contains(r#""event":"turn_routed""#), "{c}");
            assert!(c.contains(r#""flow":"DIRECT_CHAT""#));
            assert!(c.trim().ends_with('}'), "una línea JSON por evento");
        });
    }

    #[test]
    fn cada_iteracion_del_loop_queda_registrada() {
        con_dir_temporal_named("loop", |dir| {
            for i in 0..3 {
                log_react_step(i, 10, "Read", true, 5);
            }
            let c = leer(dir);
            assert_eq!(c.lines().count(), 3, "una línea por vuelta");
            assert!(c.contains(r#""max":10"#));
        });
    }

    #[test]
    fn el_final_dice_el_motivo() {
        con_dir_temporal_named("final", |dir| {
            log_turn_end("FULL_REACT", "max_iterations_exceeded", 83_000);
            let c = leer(dir);
            assert!(c.contains("max_iterations_exceeded"));
            assert!(c.contains(r#""elapsed_ms":83000"#));
        });
    }

    #[test]
    fn un_directorio_imposible_no_rompe_el_turno() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Un log que tira el chat abajo es peor que no tener log.
        std::env::set_var("NEXUM_METRICS_DIR", "/proc/imposible/nexum");
        log_turn_routed("DIRECT_CHAT", true, false, 0);
        std::env::remove_var("NEXUM_METRICS_DIR");
    }

    #[test]
    fn las_comillas_no_rompen_el_json() {
        con_dir_temporal_named("escape", |dir| {
            log_turn_provider("prov\"raro", "modelo\ncon salto");
            let c = leer(dir);
            assert!(!c.contains("\n\""), "el salto se escapa");
            assert_eq!(c.lines().count(), 1);
        });
    }
}
