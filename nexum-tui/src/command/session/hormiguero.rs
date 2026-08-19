//! /hormiguero — estado y prueba del pasillo local (Sprint 0).
//!
//! Seguridad: nunca muestra token, reasoning interno ni el nombre del
//! modelo interno (salvo NEXUM_HORMIGUERO_DEBUG=1, que además queda
//! deshabilitado en public demo por hormiguero_debug_enabled()).

use crate::{
    app::App,
    command::Command,
    hormiguero::{self, RouteOutcome},
};

pub struct HormigueroCommand;

impl Command for HormigueroCommand {
    fn name(&self) -> &str {
        "hormiguero"
    }

    fn description(&self, _lc: &crate::i18n::LcRegistry) -> String {
        // Literal a propósito: los .ftl están en obras en otra rama y este
        // comando es interno/dev-facing (evita conflicto de merge).
        "Estado del puente local Hormiguero (status | test | route)".to_string()
    }

    fn execute(&self, app: &mut App, args: &str) {
        let args = args.trim();
        match args.split_whitespace().next().unwrap_or("status") {
            "status" | "" => cmd_status(app),
            "test" => cmd_test(app),
            "route" => cmd_route(app, args.strip_prefix("route").unwrap_or("").trim()),
            "explain" => cmd_explain(app, args.strip_prefix("explain").unwrap_or("").trim()),
            "metrics" => cmd_metrics(app),
            "policy" | "profile" | "mode" => cmd_policy(app),
            other => {
                app.push_system_note(format!(
                    "/hormiguero: subcomando desconocido '{other}'. Usá: status | test | route | explain \"texto\" | metrics | policy"
                ));
            }
        }
        // Las notas se pushean fuera del pipeline: reenviar al render thread
        // (mismo patrón que /lang tras push_system_note).
        app.render_rebuild();
    }
}

fn cmd_status(app: &mut App) {
    let st = hormiguero::bridge().status();
    let enabled = if st.enabled { "on" } else { "off" };
    let alive = if st.sidecar_alive {
        "vivo"
    } else {
        "no disponible"
    };
    let model = if st.model_available { "sí" } else { "no" };
    let latency = st
        .last_latency_ms
        .map(|ms| format!("{ms}ms"))
        .unwrap_or_else(|| "-".to_string());
    let fallback = if !st.enabled || !st.sidecar_alive {
        "passthrough al proveedor principal (comportamiento normal)"
    } else {
        "no activo (pasillo operativo)"
    };
    app.push_system_note(format!(
        "Hormiguero (pasillo local)\n\
         ⎿ habilitado: {enabled} (flag NEXUM_HORMIGUERO)\n\
         ⎿ sidecar: {alive}\n\
         ⎿ modelo local disponible: {model}\n\
         ⎿ modo: {}\n\
         ⎿ circuit breaker: {} (fallos consecutivos: {})\n\
         ⎿ última latencia de ruteo: {latency}\n\
         ⎿ contadores de sesión: {} locales · {} escaladas · {} passthrough · \
{} fallos ({} timeouts) · breaker abierto {} veces\n\
         ⎿ perfil estable: {} locales · {} one-shot · {} rechazadas\n\
         ⎿ fallback: {fallback}",
        st.mode,
        st.breaker,
        st.consecutive_failures,
        st.counters.local_answers,
        st.counters.escalations,
        st.counters.passthroughs,
        st.counters.failures,
        st.counters.timeouts,
        st.counters.breaker_opens,
        st.counters.stable_local_answers,
        st.counters.stable_one_shot,
        st.counters.stable_rejected,
    ));
}

fn cmd_test(app: &mut App) {
    if !hormiguero::hormiguero_enabled() {
        app.push_system_note(
            "/hormiguero test: el pasillo está deshabilitado (NEXUM_HORMIGUERO=off). \
             Nexum funciona con el proveedor principal, como siempre."
                .to_string(),
        );
        return;
    }
    let start = std::time::Instant::now();
    let outcome = hormiguero::bridge().route_text("hola Nexum, estás?", "es");
    let elapsed = start.elapsed().as_millis();
    let note = match outcome {
        RouteOutcome::LocalAnswer(answer) => format!(
            "/hormiguero test — OK ({elapsed}ms)\n\
             ⎿ clasificación: trivial → respuesta local\n\
             ⎿ proveedor pago: NO llamado\n\
             ⎿ respuesta local: {answer}"
        ),
        RouteOutcome::NeedsPaidAi(_) => format!(
            "/hormiguero test — inesperado ({elapsed}ms): el saludo de prueba \
             se clasificó como tarea para el proveedor principal. Revisar sidecar."
        ),
        RouteOutcome::Passthrough => format!(
            "/hormiguero test — fallback ({elapsed}ms): sidecar no disponible \
             o degradado. Nexum sigue funcionando con el proveedor principal."
        ),
    };
    app.push_system_note(note);
}

fn cmd_route(app: &mut App, text: &str) {
    if !hormiguero::hormiguero_debug_enabled() {
        app.push_system_note(
            "/hormiguero route es solo para desarrollo (NEXUM_HORMIGUERO_DEBUG=1).".to_string(),
        );
        return;
    }
    if text.is_empty() {
        app.push_system_note("/hormiguero route \"texto\" — falta el texto.".to_string());
        return;
    }
    let clean = text.trim_matches('"');
    let start = std::time::Instant::now();
    let outcome = hormiguero::bridge().route_text(clean, "es");
    let elapsed = start.elapsed().as_millis();
    // Dry-run: muestra SOLO la decisión (nunca prompts ni reasoning interno).
    let decision = match outcome {
        RouteOutcome::LocalAnswer(_) => "local_answer (respondería local, sin proveedor pago)",
        RouteOutcome::NeedsPaidAi(_) => "needs_paid_ai (escalaría al proveedor principal)",
        RouteOutcome::Passthrough => "passthrough (sidecar no disponible)",
    };
    app.push_system_note(format!(
        "/hormiguero route ({elapsed}ms)\n⎿ decisión: {decision}"
    ));
}

/// /hormiguero explain "texto" — OMEGA ciclo 6b. Muestra route + reason code
/// + policy version + budgets + engine. In-process, cero red. JAMÁS muestra
/// chain-of-thought, prompts internos, memoria privada, secretos ni tokens.
fn cmd_explain(app: &mut App, text: &str) {
    if text.is_empty() {
        app.push_system_note("/hormiguero explain \"texto\" — falta el texto.".to_string());
        return;
    }
    let clean = text.trim_matches('"');
    let start = std::time::Instant::now();
    let verdict = crate::hormiguero::fastpath::classify(clean, "es");
    let elapsed_us = start.elapsed().as_micros();
    let (route, reason, worker) = match &verdict {
        crate::hormiguero::fastpath::FastVerdict::LocalAnswer(_) => {
            ("local_fast", "trivial_local", "local_fast")
        }
        crate::hormiguero::fastpath::FastVerdict::Escalate => {
            ("premium_reasoning", "fail_safe_escalated", "provider")
        }
    };
    app.push_system_note(format!(
        "/hormiguero explain ({elapsed_us}µs)\n\
         ⎿ route: {route}\n\
         ⎿ reason_code: {reason}\n\
         ⎿ policy_version: hormiguero-planning-001/v1 (corpus_v1)\n\
         ⎿ engine: deterministic (residual/planner: opt-in, fuera del hot path)\n\
         ⎿ worker: {worker}\n\
         ⎿ budgets: hot-path <5ms · plan LOW ≤10 pasos/800ms\n\
         ⎿ (sin chain-of-thought, sin contenido interno)"
    ));
}

/// /hormiguero metrics — contadores de sesión (solo números, jamás contenido).
fn cmd_metrics(app: &mut App) {
    let st = hormiguero::bridge().status();
    let c = st.counters;
    app.push_system_note(format!(
        "/hormiguero metrics (sesión)\n\
         ⎿ respuestas locales: {}\n\
         ⎿ escaladas: {}\n\
         ⎿ passthroughs: {}\n\
         ⎿ fallos sidecar: {} (timeouts: {})\n\
         ⎿ breaker opens: {}\n\
         ⎿ última latencia: {}",
        c.local_answers,
        c.escalations,
        c.passthroughs,
        c.failures,
        c.timeouts,
        c.breaker_opens,
        st.last_latency_ms
            .map(|m| format!("{m}ms"))
            .unwrap_or_else(|| "-".into()),
    ));
}

/// /hormiguero policy|profile|mode — política y perfil vigentes (estático).
fn cmd_policy(app: &mut App) {
    let enabled = hormiguero::hormiguero_enabled();
    let residual = std::env::var("NEXUM_HORMIGUERO_RESIDUAL_LLM").as_deref() == Ok("1");
    app.push_system_note(format!(
        "/hormiguero policy\n\
         ⎿ flag: {}\n\
         ⎿ perfil: LOW (validado físico; MEDIUM/POWER tipados)\n\
         ⎿ owner determinístico: Rust fastpath (CHANGE-HORMIGUERO-DETERMINISTIC-OWNER-001)\n\
         ⎿ residual LLM: {}\n\
         ⎿ policy_version: hormiguero-planning-001/v1\n\
         ⎿ principio: false local es más grave que false escalate",
        if enabled { "ON" } else { "OFF (default)" },
        if residual {
            "opt-in ACTIVO"
        } else {
            "OFF (opt-in)"
        },
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hormiguero::bridge::test_env_lock;

    #[tokio::test]
#[allow(clippy::await_holding_lock)] // test serializes env mutation via a std Mutex; keeping the guard held across await is intentional
    async fn test_hormiguero_status_renderiza_sin_crash() {
        let _guard = test_env_lock();
        std::env::remove_var("NEXUM_HORMIGUERO");
        let (mut app, _) = crate::app::App::new_headless(80, 24).await;
        HormigueroCommand.execute(&mut app, "status");
        let last = app
            .session_mgr
            .current()
            .messages
            .view_messages
            .last()
            .cloned();
        let text = match last {
            Some(crate::ui::message_view::MessageViewModel::SystemNote { content, .. }) => content,
            other => panic!("esperaba SystemNote, hay: {other:?}"),
        };
        assert!(text.contains("habilitado: off"), "reporta flag off: {text}");
        assert!(
            !text.contains("qwen"),
            "jamás expone el modelo interno: {text}"
        );
    }

    #[tokio::test]
#[allow(clippy::await_holding_lock)] // test serializes env mutation via a std Mutex; keeping the guard held across await is intentional
    async fn test_hormiguero_test_con_flag_off_informa_deshabilitado() {
        let _guard = test_env_lock();
        std::env::remove_var("NEXUM_HORMIGUERO");
        let (mut app, _) = crate::app::App::new_headless(80, 24).await;
        HormigueroCommand.execute(&mut app, "test");
        let count = app.session_mgr.current().messages.view_messages.len();
        assert!(count >= 1, "el comando siempre deja una nota");
    }

    #[tokio::test]
#[allow(clippy::await_holding_lock)] // test serializes env mutation via a std Mutex; keeping the guard held across await is intentional
    async fn test_hormiguero_route_sin_debug_es_dev_only() {
        let _guard = test_env_lock();
        std::env::remove_var("NEXUM_HORMIGUERO_DEBUG");
        let (mut app, _) = crate::app::App::new_headless(80, 24).await;
        HormigueroCommand.execute(&mut app, "route \"hola\"");
        let last = app
            .session_mgr
            .current()
            .messages
            .view_messages
            .last()
            .cloned();
        if let Some(crate::ui::message_view::MessageViewModel::SystemNote { content, .. }) = last {
            assert!(
                content.contains("desarrollo"),
                "route es dev-only: {content}"
            );
        } else {
            panic!("esperaba SystemNote");
        }
    }
}
