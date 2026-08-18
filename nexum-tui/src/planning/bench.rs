//! Modo benchmark no interactivo (OMEGA Live Wiring Fase D). Ejecuta EXACTAMENTE
//! las funciones productivas de la TUI sobre cada caso de stdin — NO una
//! implementación paralela ni classifier.py/fastpath directo:
//!   - `bridge().route_text()` (mismo hot path que `submit_message`);
//!   - `planning::gateway().request_plan` + `orchestrate_plan_steps` (mismo
//!     camino que consume el plan e inyecta scaffold);
//! y escribe evidencia real. Requiere NEXUM_HORMIGUERO=on para que el router
//! decida (con flag off, route_text es Passthrough, como en producción).

use std::io::{BufRead, Write};

/// Lee JSON lines `{"i":N,"input":"..."}` de stdin, emite
/// `{"i":N,"route":"local|escalate|passthrough","planning":"..."}` por línea.
pub fn run_bench_route() -> i32 {
    let stdin = std::io::stdin();
    let out = std::io::stdout();
    let mut lock = out.lock();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let i = v.get("i").and_then(|x| x.as_i64()).unwrap_or(-1);
        let input = v.get("input").and_then(|x| x.as_str()).unwrap_or("");

        // Routing productivo (idéntico a submit_message).
        let outcome = crate::hormiguero::bridge().route_text(input, "es");
        let route = match &outcome {
            crate::hormiguero::RouteOutcome::LocalAnswer(_) => "local",
            crate::hormiguero::RouteOutcome::NeedsPaidAi(_) => "escalate",
            crate::hormiguero::RouteOutcome::Passthrough => "passthrough",
        };

        // Planning productivo si escalada + elegible + flag on.
        let mut planning = "none";
        if route == "escalate"
            && crate::planning::planning_enabled()
            && crate::planning::is_planning_eligible(input)
        {
            let task_id = format!(
                "task-{}",
                &crate::planning::evidence::hash_text(input)[..16]
            );
            let trace_id = format!("bench-{i}");
            let tc = crate::planning::task_class_for(input);
            match crate::planning::gateway()
                .request_plan(input, tc, "low", true, &trace_id, &task_id)
            {
                crate::planning::PlanDecision::Validated(env) => {
                    crate::planning::gateway().mark_consumed(
                        &trace_id,
                        &task_id,
                        &env.plan_id,
                        &env.provenance,
                    );
                    crate::planning::orchestrate_plan_steps(&env, input, &trace_id);
                    planning = "validated";
                }
                crate::planning::PlanDecision::Rejected { .. } => planning = "rejected",
                crate::planning::PlanDecision::NeedsUserInput { .. } => {
                    planning = "needs_user_input"
                }
                crate::planning::PlanDecision::GatewayUnavailable { .. } => {
                    planning = "unavailable"
                }
            }
        }

        let _ = writeln!(
            lock,
            "{}",
            serde_json::json!({"i": i, "route": route, "planning": planning})
        );
    }
    // snapshot de métricas de planning al stderr (solo números, sin datos).
    let m = crate::planning::gateway().metrics().snapshot();
    eprintln!(
        "PLANNING_METRICS requested={} generated={} consumed={} rejected={} bypassed={} \
         validator_failed={} exec_completed={} exec_failed={} ignored_valid={}",
        m.planning_requested,
        m.planning_generated,
        m.planning_consumed,
        m.planning_rejected,
        m.planning_bypassed,
        m.validator_failed,
        m.plan_execution_completed,
        m.plan_execution_failed,
        m.ignored_valid_plans
    );
    0
}
