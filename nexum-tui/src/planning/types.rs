//! Tipos wire del `/plan` (espejo del sidecar Python) + `PlanEnvelopeV1`
//! (contrato que Rust consume). Rust sigue siendo SSOT: el envelope enriquece
//! el plan validado con autoridad de routing/permisos que el sidecar NO otorga.

use serde::{Deserialize, Serialize};

/// Respuesta cruda del endpoint `/plan` (Planner→Critic→Refiner→Validator).
/// Vista pública segura: decisión + códigos, jamás reasoning interno.
#[derive(Debug, Clone, Deserialize)]
pub struct PlanOutcomeWire {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub status: String, // "validated" | "refined_validated" | "rejected" | "needs_user_input"
    #[serde(default)]
    pub reason_code: String,
    #[serde(default)]
    pub policy_version: String,
    #[serde(default)]
    pub plan: Option<PlanWire>,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub refine_iterations: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlanWire {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub task_class: String,
    #[serde(default)]
    pub route: String,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub steps: Vec<PlanStepWire>,
    #[serde(default)]
    pub expected_evidence: Vec<String>,
    #[serde(default)]
    pub risk: String,
    #[serde(default)]
    pub tool_intents: Vec<String>,
    #[serde(default)]
    pub worker_hints: Vec<String>,
    #[serde(default)]
    pub budget: BudgetWire,
    #[serde(default)]
    pub stop_conditions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlanStepWire {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub tool: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct BudgetWire {
    #[serde(default)]
    pub max_steps: u32,
    #[serde(default)]
    pub max_latency_ms: u64,
    #[serde(default)]
    pub max_refine_iterations: u32,
}

// ── PlanEnvelopeV1 (lo que Rust consume; autoridad de permisos = Rust) ──

/// Un paso ordenado del plan, con la capability y el gobierno de aprobación
/// que **decide Rust** (el sidecar propone; Rust dispone).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanStepV1 {
    pub id: String,
    pub action: String,
    /// Capability = tool explícito o, si no, el `kind` del paso.
    pub capability: String,
    pub risk_class: String,
    /// Aprobación requerida — determinada por Rust según risk/capability.
    pub required_approval: bool,
    pub dependencies: Vec<String>,
    pub expected_evidence: Vec<String>,
}

/// PlanEnvelopeV1: contrato mínimo que exige la remediación. Producido por Rust
/// a partir del plan validado por el sidecar. Nunca contiene reasoning interno.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanEnvelopeV1 {
    pub schema_version: u32,
    pub plan_id: String,
    pub task_id: String,
    pub task_class: String,
    pub risk_class: String,
    pub required_approval: bool,
    pub ordered_steps: Vec<PlanStepV1>,
    pub expected_evidence: Vec<String>,
    pub budgets: BudgetWire,
    /// Deadline absoluto en ms derivado del presupuesto de latencia.
    pub deadline_ms: u64,
    pub stop_conditions: Vec<String>,
    /// Provenance: de dónde salió y bajo qué política. Sin datos de usuario.
    pub provenance: String,
    /// `validated` | `refined_validated`.
    pub validation_status: String,
    pub policy_version: String,
    pub refine_iterations: u32,
}

/// Resultado de pedir un plan al gateway. Determinístico y total.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanDecision {
    /// Plan generado y validado por el Validator determinístico. Rust lo consume.
    Validated(Box<PlanEnvelopeV1>),
    /// El Validator (o el Critic con hallazgo fatal) rechazó el plan. Fail-closed.
    Rejected { reason_code: String, validator_failed: bool },
    /// El planner necesita más información del usuario. No se ejecuta nada.
    NeedsUserInput { reason_code: String },
    /// Sidecar caído/lento/inválido/flag off. El caller decide (bypass si el
    /// plan NO era obligatorio; fail-closed si lo era).
    GatewayUnavailable { detail: String },
}

impl PlanEnvelopeV1 {
    /// Resumen compacto e inyectable como scaffold de ejecución para el agente.
    /// Solo estructura del plan validado — sin reasoning interno ni datos crudos.
    pub fn execution_scaffold(&self) -> String {
        let mut s = String::new();
        s.push_str("<nexum-plan validated=\"true\">\n");
        s.push_str(&format!(
            "plan_id={} risk={} approval_required={} policy={}\n",
            self.plan_id, self.risk_class, self.required_approval, self.policy_version
        ));
        s.push_str("Plan validado por el Validator determinístico. Seguí los pasos en orden:\n");
        for (i, step) in self.ordered_steps.iter().enumerate() {
            let dep = if step.dependencies.is_empty() {
                String::new()
            } else {
                format!(" (tras {})", step.dependencies.join(","))
            };
            s.push_str(&format!(
                "  {}. [{}] {}{}\n",
                i + 1,
                step.capability,
                step.action,
                dep
            ));
        }
        if !self.expected_evidence.is_empty() {
            s.push_str(&format!(
                "Evidencia esperada: {}\n",
                self.expected_evidence.join("; ")
            ));
        }
        if !self.stop_conditions.is_empty() {
            s.push_str(&format!("Condiciones de parada: {}\n", self.stop_conditions.join("; ")));
        }
        s.push_str("Rust conserva autoridad sobre tools, permisos y providers; HITL sigue vigente.\n");
        s.push_str("</nexum-plan>");
        s
    }
}
