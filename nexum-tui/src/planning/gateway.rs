//! PlanningGateway: interfaz acotada entre el runtime Rust (SSOT) y el
//! Planner→Critic→Refiner→Validator del sidecar. Rust decide trivialidad/riesgo
//! y elegibilidad; el sidecar propone y valida; Rust consume el plan validado y
//! conserva autoridad sobre routing, tools, permisos y providers.
//!
//! No crea un segundo router: solo pide un plan para tareas planificables que
//! Rust YA decidió escalar. Fail-closed si el plan obligatorio no valida.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::metrics::PlanningMetrics;
use super::metrics::PlanningMetrics as M;
use super::types::*;

/// Presupuesto de la llamada de planificación (fuera del hot path trivial;
/// solo corre en tareas ya escaladas). Estricto para no colgar la TUI.
const PLAN_BUDGET: Duration = Duration::from_millis(2500);

pub struct PlanningGateway {
    metrics: PlanningMetrics,
}

static GATEWAY: OnceLock<PlanningGateway> = OnceLock::new();

pub fn gateway() -> &'static PlanningGateway {
    GATEWAY.get_or_init(|| PlanningGateway {
        metrics: PlanningMetrics::default(),
    })
}

impl PlanningGateway {
    pub fn metrics(&self) -> &PlanningMetrics {
        &self.metrics
    }

    /// Pide, valida y encapsula un plan para un pedido planificable ya escalado.
    ///
    /// `required` = el plan es obligatorio para esta tarea (fail-closed si falla).
    /// Devuelve un `PlanDecision` total: el caller nunca queda sin veredicto.
    pub fn request_plan(
        &self,
        text: &str,
        task_class: &str,
        profile: &str,
        required: bool,
        trace_id: &str,
        task_id: &str,
    ) -> PlanDecision {
        M::inc(&self.metrics.planning_requested);

        // Descubrimiento: mismo runtime_dir/token que el bridge (SSOT único).
        let Some((port, token)) = super::discover_sidecar() else {
            return self.on_unavailable("sidecar no descubrible", required);
        };

        let body = serde_json::json!({
            "text": text,
            "task_class": task_class,
            "profile": profile,
        })
        .to_string();

        let start = Instant::now();
        let resp = crate::hormiguero::http::request(
            port,
            "POST",
            "/plan",
            Some(&token),
            Some(&body),
            PLAN_BUDGET,
        );
        let _elapsed = start.elapsed();

        let outcome: PlanOutcomeWire = match resp {
            Ok(r) if r.status == 200 => match serde_json::from_str(&r.body) {
                Ok(o) => o,
                Err(e) => return self.on_unavailable(&format!("parse: {e}"), required),
            },
            Ok(r) => return self.on_unavailable(&format!("http {}", r.status), required),
            Err(e) => return self.on_unavailable(&format!("transporte: {e}"), required),
        };

        if !outcome.ok {
            return self.on_unavailable("outcome.ok=false", required);
        }

        match outcome.status.as_str() {
            "validated" | "refined_validated" => {
                let Some(plan) = outcome.plan else {
                    // Estado válido sin plan ⇒ tratamos como rechazo (fail-closed).
                    // El plan se "generó" (status validado) pero no se puede consumir.
                    M::inc(&self.metrics.planning_generated);
                    M::inc(&self.metrics.planning_rejected);
                    M::inc(&self.metrics.validator_failed);
                    return PlanDecision::Rejected {
                        reason_code: "validated_without_plan".into(),
                        validator_failed: true,
                    };
                };
                let envelope = self.encapsulate(
                    plan,
                    &outcome.status,
                    &outcome.reason_code,
                    &outcome.policy_version,
                    outcome.refine_iterations,
                    task_id,
                );
                M::inc(&self.metrics.planning_generated);
                // Rust consume el plan: registra generación en evidencia.
                super::evidence::record(&super::evidence::EvidenceEvent {
                    trace_id,
                    task_id,
                    plan_id: Some(&envelope.plan_id),
                    lifecycle: "plan_generated",
                    component: "planning-gateway",
                    provenance: &envelope.provenance,
                    input_hash: &super::evidence::hash_text(text),
                    output_hash: &super::evidence::hash_text(&envelope.plan_id),
                });
                super::evidence::record(&super::evidence::EvidenceEvent {
                    trace_id,
                    task_id,
                    plan_id: Some(&envelope.plan_id),
                    lifecycle: "plan_validated",
                    component: "validator",
                    provenance: &envelope.provenance,
                    input_hash: &super::evidence::hash_text(&envelope.plan_id),
                    output_hash: "",
                });
                PlanDecision::Validated(Box::new(envelope))
            }
            "rejected" => {
                // Un plan fue producido por el planner y luego rechazado por
                // critic/validator ⇒ cuenta como generado (invariante:
                // consumed = generated - rejected) y como rechazado.
                M::inc(&self.metrics.planning_generated);
                M::inc(&self.metrics.planning_rejected);
                // El rechazo con errores del Validator cuenta como validator_failed.
                let validator_failed = !outcome.errors.is_empty();
                if validator_failed {
                    M::inc(&self.metrics.validator_failed);
                }
                super::evidence::record(&super::evidence::EvidenceEvent {
                    trace_id,
                    task_id,
                    plan_id: None,
                    lifecycle: "plan_rejected",
                    component: "validator",
                    provenance: &format!("hormiguero-planning/{}", outcome.policy_version),
                    input_hash: &super::evidence::hash_text(text),
                    output_hash: &super::evidence::hash_text(&outcome.reason_code),
                });
                PlanDecision::Rejected {
                    reason_code: outcome.reason_code,
                    validator_failed,
                }
            }
            "needs_user_input" => PlanDecision::NeedsUserInput {
                reason_code: outcome.reason_code,
            },
            other => self.on_unavailable(&format!("status desconocido: {other}"), required),
        }
    }

    /// Marca que un plan validado fue efectivamente consumido por el runtime.
    pub fn mark_consumed(&self, trace_id: &str, task_id: &str, plan_id: &str, provenance: &str) {
        M::inc(&self.metrics.planning_consumed);
        super::evidence::record(&super::evidence::EvidenceEvent {
            trace_id,
            task_id,
            plan_id: Some(plan_id),
            lifecycle: "plan_consumed",
            component: "runtime",
            provenance,
            input_hash: "",
            output_hash: "",
        });
    }

    /// Un plan válido que NO se consumió es una violación de invariante (gate=0).
    pub fn mark_ignored_valid_plan(&self) {
        M::inc(&self.metrics.ignored_valid_plans);
    }

    pub fn mark_execution_completed(&self) {
        M::inc(&self.metrics.plan_execution_completed);
    }

    pub fn mark_execution_failed(&self) {
        M::inc(&self.metrics.plan_execution_failed);
    }

    fn on_unavailable(&self, detail: &str, required: bool) -> PlanDecision {
        // Si el plan NO era obligatorio, el caller hace bypass (métrica).
        // Si era obligatorio, el caller hace fail-closed; acá solo reportamos.
        if !required {
            M::inc(&self.metrics.planning_bypassed);
        }
        PlanDecision::GatewayUnavailable {
            detail: detail.to_string(),
        }
    }

    /// Mapea el plan validado del sidecar a PlanEnvelopeV1, aplicando la
    /// autoridad de Rust: la aprobación requerida la decide Rust por risk/capability,
    /// NUNCA el sidecar. La IA premium jamás obtiene permisos.
    fn encapsulate(
        &self,
        plan: PlanWire,
        status: &str,
        reason_code: &str,
        policy_version: &str,
        refine_iterations: u32,
        task_id: &str,
    ) -> PlanEnvelopeV1 {
        let plan_hash = super::evidence::hash_text(&format!(
            "{task_id}|{}|{}|{}",
            plan.task_class,
            plan.risk,
            plan.steps.len()
        ));
        let plan_id = format!("plan-{}", &plan_hash[..16.min(plan_hash.len())]);

        let ordered_steps: Vec<PlanStepV1> = plan
            .steps
            .iter()
            .map(|s| {
                let capability = s
                    .tool
                    .clone()
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| s.kind.clone());
                PlanStepV1 {
                    id: s.id.clone(),
                    action: s.action.clone(),
                    required_approval: Self::approval_for(&plan.risk, &capability),
                    capability,
                    risk_class: plan.risk.clone(),
                    dependencies: s.depends_on.clone(),
                    expected_evidence: s.evidence.clone(),
                }
            })
            .collect();

        let required_approval = ordered_steps.iter().any(|s| s.required_approval)
            || Self::approval_for(&plan.risk, "");

        PlanEnvelopeV1 {
            schema_version: plan.schema_version,
            plan_id,
            task_id: task_id.to_string(),
            task_class: plan.task_class,
            risk_class: plan.risk,
            required_approval,
            ordered_steps,
            expected_evidence: plan.expected_evidence,
            deadline_ms: plan.budget.max_latency_ms,
            budgets: plan.budget,
            stop_conditions: plan.stop_conditions,
            provenance: format!("hormiguero-planning/{policy_version}"),
            validation_status: status.to_string(),
            policy_version: policy_version.to_string(),
            refine_iterations,
        }
        .also_reason(reason_code)
    }

    /// Política determinística de aprobación (autoridad Rust). Riesgo elevado o
    /// capabilities de escritura/ejecución exigen HITL. Conservador por defecto.
    fn approval_for(risk: &str, capability: &str) -> bool {
        let risky_risk = matches!(risk, "elevated" | "high" | "critical");
        let write_cap = matches!(
            capability,
            "write" | "edit" | "execute" | "bash" | "shell" | "delete" | "network" | "MUTATE"
        ) || capability.eq_ignore_ascii_case("write")
            || capability.eq_ignore_ascii_case("mutate");
        risky_risk || write_cap
    }
}

impl PlanEnvelopeV1 {
    /// No-op semántico: el reason_code ya viaja en la evidencia; se conserva el
    /// envelope tal cual. (Placeholder explícito para trazabilidad futura.)
    fn also_reason(self, _reason_code: &str) -> Self {
        self
    }
}

#[cfg(test)]
#[path = "gateway_test.rs"]
mod gateway_test;
