//! Planning: integración del Planner→Critic→Refiner→Validator al camino vivo.
//!
//! Rust es SSOT del routing y el control plane. Este módulo NO crea un segundo
//! router: solo, para tareas que Rust ya decidió escalar y clasificó como
//! planificables, pide un plan al sidecar, deja que el Validator determinístico
//! lo gobierne, y consume el plan validado. Plan obligatorio inválido ⇒ fail-closed.
//!
//! Flag maestro `NEXUM_PLANNING` (OFF por defecto, forzado OFF en public demo).

pub mod bench;
pub mod cartero;
pub mod evidence;
pub mod gateway;
pub mod metrics;
pub mod types;
pub mod workers;

pub use gateway::{gateway, PlanningGateway};
pub use metrics::PlanningCounters;
pub use types::{PlanDecision, PlanEnvelopeV1};

/// Flag maestro de planning. Default OFF. Public demo lo fuerza OFF siempre.
pub fn planning_enabled() -> bool {
    if crate::ui::demo_mode::public_demo_enabled() {
        return false;
    }
    matches!(
        std::env::var("NEXUM_PLANNING")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "on" | "1" | "true" | "yes"
    )
}

/// Descubre (puerto, token) del sidecar — MISMO runtime_dir/archivos que el
/// bridge del Hormiguero (un solo sidecar por sesión, SSOT único).
pub(crate) fn discover_sidecar() -> Option<(u16, String)> {
    let dir = crate::hormiguero::bridge::runtime_dir()?;
    let port: u16 = std::fs::read_to_string(dir.join("hormiguero.port"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let token = std::fs::read_to_string(dir.join("hormiguero.token"))
        .ok()?
        .trim()
        .to_string();
    if token.is_empty() {
        return None;
    }
    Some((port, token))
}

/// ¿Esta tarea (ya escalada) exige planificación? Política determinística de
/// Rust: tareas triviales/seguras las resuelve el fastpath y no llegan acá;
/// las que involucran múltiples pasos, código o herramientas SÍ requieren plan.
///
/// Conservador: ante la duda, elegible (mejor planificar de más que ejecutar
/// a ciegas). El texto NO se loggea.
pub fn is_planning_eligible(text: &str) -> bool {
    let t = text.to_lowercase();
    let words = t.split_whitespace().count();
    // Muy corto y sin verbos de acción ⇒ probablemente no planificable.
    if words < 3 {
        return false;
    }
    const TRIGGERS: &[&str] = &[
        // código / build
        "código", "codigo", "code", "función", "funcion", "function", "script",
        "programa", "implementá", "implementa", "implement", "refactor", "test",
        "compilá", "compila", "build", "bug", "fix", "arreglá", "arregla",
        // multi-paso / archivos / sistema
        "paso", "pasos", "step", "primero", "luego", "después", "despues", "then",
        "archivo", "file", "carpeta", "directorio", "crear", "create", "escribí",
        "escribi", "write", "modificá", "modifica", "edit", "borrá", "delete",
        "instalá", "instala", "install", "configurá", "configura", "deploy",
        // análisis / investigación
        "analizá", "analiza", "analyze", "investigá", "investiga", "research",
        "buscá", "busca", "search", "comparar", "compare", "auditá", "audita",
    ];
    TRIGGERS.iter().any(|k| t.contains(k))
}

/// Resumen del despacho de pasos del plan (para métricas/evidencia).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlanExecSummary {
    pub steps_total: usize,
    pub dispatched: usize,
    pub deferred_to_hitl: usize,
    pub errors: usize,
}

/// Orquesta la ejecución de los pasos de un plan validado bajo autoridad de Rust:
/// Rust selecciona el paso → Cartero construye contexto mínimo tipado → Worker
/// recibe capability acotada → Rust autoriza (solo-lectura) o difiere a HITL
/// (escritura/exec/red) → resultado tipado → Evidence. NO ejecuta tools (eso es
/// del agente con HITL); produce la traza de gobernanza + evidencia real.
pub fn orchestrate_plan_steps(
    env: &PlanEnvelopeV1,
    raw_input: &str,
    trace_id: &str,
) -> PlanExecSummary {
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut sum = PlanExecSummary {
        steps_total: env.ordered_steps.len(),
        ..Default::default()
    };
    for step in &env.ordered_steps {
        let ctx = cartero::build_step_context(step, env, raw_input);
        let read_only =
            ctx.scope.iter().all(|s| s == "fs:read" || s == "ctx:read") && !step.required_approval;
        let request_id = format!("{}#{}", env.plan_id, step.id);
        if read_only {
            match workers::registry().dispatch(
                "plan_step",
                "read",
                &request_id,
                ctx,
                cancel.clone(),
                true, // Rust autoriza el paso de solo-lectura
            ) {
                Ok(out) => {
                    sum.dispatched += 1;
                    evidence::record(&evidence::EvidenceEvent {
                        trace_id,
                        task_id: &env.task_id,
                        plan_id: Some(&env.plan_id),
                        lifecycle: "worker_result",
                        component: "worker:plan_step",
                        provenance: &env.provenance,
                        input_hash: &evidence::hash_text(&request_id),
                        output_hash: &evidence::hash_text(
                            out.output.get("status").map(|s| s.as_str()).unwrap_or(""),
                        ),
                    });
                }
                Err(e) => {
                    sum.errors += 1;
                    evidence::record(&evidence::EvidenceEvent {
                        trace_id,
                        task_id: &env.task_id,
                        plan_id: Some(&env.plan_id),
                        lifecycle: "worker_error",
                        component: "worker:plan_step",
                        provenance: &env.provenance,
                        input_hash: &evidence::hash_text(&request_id),
                        output_hash: e.code(),
                    });
                }
            }
        } else {
            // Escritura/exec/red o riesgo ⇒ Rust difiere a HITL, no auto-ejecuta.
            sum.deferred_to_hitl += 1;
            evidence::record(&evidence::EvidenceEvent {
                trace_id,
                task_id: &env.task_id,
                plan_id: Some(&env.plan_id),
                lifecycle: "step_requires_approval",
                component: "runtime",
                provenance: &env.provenance,
                input_hash: &evidence::hash_text(&request_id),
                output_hash: &ctx.capability,
            });
        }
    }
    if sum.errors == 0 {
        gateway().mark_execution_completed();
    } else {
        gateway().mark_execution_failed();
    }
    sum
}

/// Clase de tarea heurística para el planner (determinística, sin red).
pub fn task_class_for(text: &str) -> &'static str {
    let t = text.to_lowercase();
    let code = ["código", "codigo", "code", "función", "funcion", "function",
                "script", "programa", "refactor", "bug", "fix", "compilá", "build"];
    if code.iter().any(|k| t.contains(k)) {
        return "code";
    }
    "generic"
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod mod_test;
