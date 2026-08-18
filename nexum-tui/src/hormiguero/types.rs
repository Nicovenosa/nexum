//! Tipos wire del pasillo (espejo de los endpoints del sidecar) + TaskEnvelope.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub uptime_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusResponse {
    pub ok: bool,
    #[serde(default)]
    pub hormiguero_available: bool,
    #[serde(default)]
    pub model_available: bool,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Respuesta del endpoint `/route` del sidecar. Desde OMEGA Fase 4 el hot
/// path decide in-process (`fastpath`) y no llama a `/route`; este tipo queda
/// reservado para el camino async del sidecar (envelope/planner, Fase 7).
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct RouteResponse {
    pub ok: bool,
    #[serde(default)]
    pub decision: String, // "local_answer" | "needs_paid_ai"
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub should_call_paid_ai: bool,
    #[serde(default)]
    pub task_envelope: Option<TaskEnvelope>,
    #[serde(default)]
    pub confidence: f32,
}

/// TaskEnvelope (contrato completo desde Sprint 0; uso real como prompt: F1).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TaskEnvelope {
    /// Identidades generadas una vez al clasificar el turno. No se regeneran
    /// en ACP ni se derivan del texto del prompt.
    #[serde(default)]
    pub envelope_id: String,
    #[serde(default)]
    pub trace_id: String,
    #[serde(default)]
    pub turn_id: String,
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub route_decision: StableRouteDecision,
    #[serde(default)]
    pub task_classification: StableTaskClassification,
    #[serde(default)]
    pub user_intent: String,
    #[serde(default)]
    pub normalized_request: String,
    #[serde(default)]
    pub relevant_context_summary: String,
    #[serde(default)]
    pub selected_project_context: Vec<serde_json::Value>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub safety_notes: Vec<String>,
    #[serde(default)]
    pub redaction_report: Option<RedactionReport>,
    #[serde(default)]
    pub required_output_format: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    #[serde(default)]
    pub escalation_reason: String,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub metrics: Option<EnvelopeMetrics>,
}

/// Decisión final del clasificador para el perfil estable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StableRouteDecision {
    #[default]
    Unspecified,
    OneShot,
    RejectedByPolicy,
}

/// Clasificación semántica producida junto con la decisión. ACP sólo la
/// valida; nunca vuelve a inferirla desde el prompt.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StableTaskClassification {
    #[default]
    Unspecified,
    Simple,
    Advanced,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RedactionReport {
    #[serde(default)]
    pub secrets_redacted: u32,
    #[serde(default)]
    pub paths_normalized: u32,
    #[serde(default)]
    pub dropped_sections: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct EnvelopeMetrics {
    #[serde(default)]
    pub naive_size_bytes: u64,
    #[serde(default)]
    pub envelope_size_bytes: u64,
    #[serde(default)]
    pub envelope_naive_ratio: Option<f64>,
}

/// Resultado del pre-route para `submit_message`.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteOutcome {
    /// Respuesta local lista — NO llamar al provider pago.
    LocalAnswer(String),
    /// Escalar: el input sigue al provider (uso del envelope como prompt:
    /// F1 del pasillo). El envelope viaja para clientes que lo necesiten
    /// (voz: mostrar/medir sin re-pedir al sidecar).
    NeedsPaidAi(Option<Box<TaskEnvelope>>),
    /// Sidecar off/lento/inválido/flag off → flujo actual sin cambios.
    Passthrough,
}

/// Estado del breaker + última latencia (para /hormiguero status).
#[derive(Debug, Clone)]
pub struct BridgeStatus {
    pub enabled: bool,
    pub sidecar_alive: bool,
    pub model_available: bool,
    pub mode: String,
    pub breaker: BreakerState,
    pub last_latency_ms: Option<u64>,
    pub consecutive_failures: u32,
    pub counters: BridgeCounters,
}

/// Contadores acumulados de la sesión (observabilidad mínima H-2 / G-2).
/// Solo números: jamás texto de usuario, tokens ni bodies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BridgeCounters {
    /// Triviales respondidas localmente por el sidecar.
    pub local_answers: u64,
    /// Escaladas al provider pago (needs_paid_ai).
    pub escalations: u64,
    /// Respuestas locales clasificadas por la ruta estable obligatoria.
    pub stable_local_answers: u64,
    /// Turnos estables clasificados ONE_SHOT.
    pub stable_one_shot: u64,
    /// Turnos estables rechazados antes del provider.
    pub stable_rejected: u64,
    /// Passthroughs con flag on (breaker abierto, sidecar apagado o fallo).
    pub passthroughs: u64,
    /// Fallos de la llamada al sidecar (HTTP != 200, parse, transporte).
    pub failures: u64,
    /// Subconjunto de fallos con presupuesto agotado (>= ROUTE_BUDGET).
    pub timeouts: u64,
    /// Veces que el breaker transicionó a OPEN.
    pub breaker_opens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

impl std::fmt::Display for BreakerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BreakerState::Closed => write!(f, "closed (ok)"),
            BreakerState::Open => write!(f, "open (degradado, passthrough)"),
            BreakerState::HalfOpen => write!(f, "half-open (probando)"),
        }
    }
}
