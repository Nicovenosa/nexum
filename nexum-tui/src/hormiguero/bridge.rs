//! Bridge fino: discovery del sidecar + budget + circuit breaker + fallback.

use std::path::PathBuf;
#[cfg(test)]
use std::sync::MutexGuard;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::http;
use super::types::*;

/// Budget de las llamadas de status/health (comando /hormiguero status).
/// El pre-route ya no hace red (OMEGA Fase 4): decide in-process en `fastpath`.
const STATUS_BUDGET: Duration = Duration::from_millis(1500);
/// Fallos consecutivos que abren el breaker.
const BREAKER_THRESHOLD: u32 = 3;
/// Tiempo que el breaker queda abierto antes de half-open.
const BREAKER_OPEN_SECS: u64 = 30;
/// TTL del cache del probe de status (OMEGA Fase 9, cierra H-3): status
/// repetido NO re-golpea sidecar/Ollama dentro de la ventana. Resultados
/// negativos también se cachean (sidecar muerto ⇒ un connect por TTL, no
/// uno por consulta). Override solo para tests: NEXUM_HORMIGUERO_STATUS_TTL_MS.
const STATUS_CACHE_TTL: Duration = Duration::from_millis(5000);

fn status_cache_ttl() -> Duration {
    match std::env::var("NEXUM_HORMIGUERO_STATUS_TTL_MS") {
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms.min(60_000)),
            Err(_) => STATUS_CACHE_TTL,
        },
        Err(_) => STATUS_CACHE_TTL,
    }
}

pub struct Bridge {
    inner: Mutex<Inner>,
}

/// Resultado del último probe de status (positivo o negativo) + timestamp.
struct StatusProbe {
    at: Instant,
    alive: bool,
    model_available: bool,
    mode: String,
}

#[derive(Default)]
struct Inner {
    consecutive_failures: u32,
    open_until: Option<Instant>,
    last_latency_ms: Option<u64>,
    counters: BridgeCounters,
    status_probe: Option<StatusProbe>,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

/// Singleton del proceso (un solo sidecar por sesión de TUI).
pub fn bridge() -> &'static Bridge {
    BRIDGE.get_or_init(|| Bridge {
        inner: Mutex::new(Inner::default()),
    })
}

/// Lock global para tests que mutan env vars — delega en el lock ÚNICO
/// compartido del crate (demo_mode) para serializar contra los tests de
/// public_demo/predictions que tocan las mismas variables.
#[cfg(test)]
pub fn test_env_lock() -> MutexGuard<'static, ()> {
    crate::ui::demo_mode::test_env_lock()
}

/// Directorio de runtime — MISMA prioridad que el sidecar Python:
/// env explícita (el launcher la exporta siempre) > $XDG_RUNTIME_DIR/nexum.
/// Sin ninguna de las dos ⇒ sidecar no descubrible ⇒ passthrough.
/// (pub(crate) para voice F2: mismo runtime dir que el sidecar.)
pub(crate) fn runtime_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("NEXUM_HORMIGUERO_RUNTIME_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            // Aislado por PID bajo test, igual que
            // nexum_acp::transport::local::local_runtime_directory.
            //
            // Este era el SEGUNDO resolver del runtime dir y se me pasó al
            // aislar el primero: dos lugares para la misma verdad, dentro del
            // arreglo que existía para cerrar esa clase. La evidencia fue
            // /run/user/1000/nexum/voice-last-error.txt escrito por una corrida
            // de tests en el runtime dir REAL del usuario.
            let sub = if nexum_agent::sandbox::running_under_test() {
                format!("nexum-{}", nexum_agent::sandbox::session_suffix())
            } else {
                "nexum".to_string()
            };
            return Some(PathBuf::from(xdg).join(sub));
        }
    }
    None
}

/// Lee (puerto, token) publicados por el sidecar. El token jamás se loggea.
fn discover() -> Option<(u16, String)> {
    let dir = runtime_dir()?;
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

impl Bridge {
    /// Ruta obligatoria del perfil estable. Es completamente in-process y no
    /// depende del flag/sidecar: produce un envelope para cada variante que
    /// continúa hacia ACP.
    pub fn route_stable_text(&self, text: &str, locale: &str) -> RouteOutcome {
        use super::fastpath::StableFastVerdict;

        let start = Instant::now();
        let outcome = match super::fastpath::classify_stable(text, locale) {
            StableFastVerdict::LocalAnswer(answer) => {
                self.bump(|c| c.stable_local_answers += 1);
                RouteOutcome::LocalAnswer(answer)
            }
            verdict @ (StableFastVerdict::OneShot | StableFastVerdict::RejectedByPolicy) => {
                self.bump(|c| match verdict {
                    StableFastVerdict::OneShot => c.stable_one_shot += 1,
                    StableFastVerdict::RejectedByPolicy => c.stable_rejected += 1,
                    StableFastVerdict::LocalAnswer(_) => unreachable!(),
                });
                let id = uuid::Uuid::now_v7().to_string();
                let (route_decision, task_classification, escalation_reason) = match verdict {
                    StableFastVerdict::OneShot => (
                        StableRouteDecision::OneShot,
                        StableTaskClassification::Simple,
                        "stable_one_shot",
                    ),
                    StableFastVerdict::RejectedByPolicy => (
                        StableRouteDecision::RejectedByPolicy,
                        StableTaskClassification::Advanced,
                        "stable_advanced_task",
                    ),
                    StableFastVerdict::LocalAnswer(_) => unreachable!(),
                };
                RouteOutcome::NeedsPaidAi(Some(Box::new(TaskEnvelope {
                    envelope_id: id.clone(),
                    trace_id: format!("trace-{id}"),
                    turn_id: format!("turn-{id}"),
                    request_id: format!("request-{id}"),
                    route_decision,
                    task_classification,
                    user_intent: text.trim().to_string(),
                    normalized_request: text.trim().to_string(),
                    required_output_format: "text".to_string(),
                    escalation_reason: escalation_reason.to_string(),
                    confidence: 1.0,
                    source: "tui".to_string(),
                    ..TaskEnvelope::default()
                })))
            }
        };
        self.record_success(start.elapsed().as_millis() as u64);
        outcome
    }

    /// Pre-route del input del usuario en el **hot path** (thread de UI).
    ///
    /// OMEGA Fase 4 (cierre H-1): decide **in-process, cero red, en
    /// microsegundos**. Antes hacía una llamada HTTP sync al sidecar y podía
    /// bloquear la TUI hasta ~800ms si el sidecar colgaba. Ahora el sidecar
    /// jamás se toca desde el hot path — el router determinístico de
    /// `fastpath` (espejo de la capa determinística de classifier.py) resuelve
    /// triviales localmente y escala todo lo demás. NUNCA panic, NUNCA red.
    pub fn route_text(&self, text: &str, _locale: &str) -> RouteOutcome {
        if !super::hormiguero_enabled() {
            return RouteOutcome::Passthrough;
        }
        let start = Instant::now();
        let outcome = match super::fastpath::classify(text, _locale) {
            super::fastpath::FastVerdict::LocalAnswer(answer) => {
                self.bump(|c| c.local_answers += 1);
                RouteOutcome::LocalAnswer(answer)
            }
            super::fastpath::FastVerdict::Escalate => {
                self.bump(|c| c.escalations += 1);
                // Sin envelope en el hot path: la construcción del envelope
                // (sidecar) es un camino async fuera del thread de UI (Fase 7).
                RouteOutcome::NeedsPaidAi(None)
            }
        };
        // El hot path in-process no puede fallar por red: registra latencia
        // (sub-ms) para /hormiguero status sin abrir el breaker.
        self.record_success(start.elapsed().as_millis() as u64);
        outcome
    }

    /// Incremento atómico de contadores (observabilidad H-2, sin datos).
    fn bump(&self, f: impl FnOnce(&mut BridgeCounters)) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut inner.counters);
    }

    /// Estado para /hormiguero status (usa budget más laxo, fuera del hot path).
    /// Cache TTL (H-3): consultas repetidas no re-tocan la red dentro de la
    /// ventana; el resultado negativo también se cachea.
    pub fn status(&self) -> BridgeStatus {
        let enabled = super::hormiguero_enabled();
        let mut alive = false;
        let mut model_available = false;
        let mut mode = "-".to_string();
        // 1. Cache vigente ⇒ cero red, cero locks largos.
        let mut cache_hit = false;
        if enabled {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(probe) = &inner.status_probe {
                if probe.at.elapsed() < status_cache_ttl() {
                    alive = probe.alive;
                    model_available = probe.model_available;
                    mode = probe.mode.clone();
                    cache_hit = true;
                }
            }
        }
        // 2. El probe de salud sí toca red (fuera del hot path) y alimenta el
        // breaker: si transiciona a OPEN, el próximo /status lo salta.
        if enabled && !cache_hit && self.breaker_allows() {
            if let Some((port, token)) = discover() {
                let start = Instant::now();
                match http::request(port, "GET", "/health", None, None, STATUS_BUDGET) {
                    Ok(resp) if resp.status == 200 => {
                        alive = true;
                        self.record_success(start.elapsed().as_millis() as u64);
                    }
                    other => {
                        self.bump(|c| {
                            c.failures += 1;
                            if start.elapsed() >= STATUS_BUDGET {
                                c.timeouts += 1;
                            }
                        });
                        self.record_failure();
                        let _ = other;
                    }
                }
                if alive {
                    if let Ok(resp) =
                        http::request(port, "GET", "/status", Some(&token), None, STATUS_BUDGET)
                    {
                        if resp.status == 200 {
                            if let Ok(st) = serde_json::from_str::<StatusResponse>(&resp.body) {
                                model_available = st.model_available;
                                mode = st.mode;
                            }
                        }
                    }
                }
                // Cachear el resultado del probe (positivo O negativo).
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.status_probe = Some(StatusProbe {
                    at: Instant::now(),
                    alive,
                    model_available,
                    mode: mode.clone(),
                });
            }
        }
        // Snapshot DESPUÉS del probe: refleja el estado de este /status,
        // incluida la transición del breaker que el probe pudo disparar.
        let (breaker, failures, last_latency, counters) = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            (
                self.breaker_state_locked(&inner),
                inner.consecutive_failures,
                inner.last_latency_ms,
                inner.counters,
            )
        };
        BridgeStatus {
            enabled,
            sidecar_alive: alive,
            model_available,
            mode,
            breaker,
            last_latency_ms: last_latency,
            consecutive_failures: failures,
            counters,
        }
    }

    // ── breaker ──────────────────────────────────────────────────────

    fn breaker_allows(&self) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match inner.open_until {
            Some(until) if Instant::now() < until => false,
            Some(_) => {
                // half-open: se permite UN intento; si falla vuelve a abrir.
                inner.open_until = None;
                true
            }
            None => true,
        }
    }

    fn record_success(&self, latency_ms: u64) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.consecutive_failures = 0;
        inner.open_until = None;
        inner.last_latency_ms = Some(latency_ms);
    }

    fn record_failure(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.consecutive_failures += 1;
        if inner.consecutive_failures >= BREAKER_THRESHOLD {
            let reopening = inner.open_until.is_none();
            inner.open_until = Some(Instant::now() + Duration::from_secs(BREAKER_OPEN_SECS));
            if reopening {
                inner.counters.breaker_opens += 1;
                // Único log de la transición (sin spam, sin datos sensibles).
                tracing::warn!(
                    failures = inner.consecutive_failures,
                    "hormiguero bridge: breaker OPEN, passthrough temporal"
                );
            }
        }
    }

    fn breaker_state_locked(&self, inner: &Inner) -> BreakerState {
        match inner.open_until {
            Some(until) if Instant::now() < until => BreakerState::Open,
            Some(_) => BreakerState::HalfOpen,
            None if inner.consecutive_failures > 0 => BreakerState::Closed,
            None => BreakerState::Closed,
        }
    }

    /// Solo tests: resetear estado del breaker.
    #[cfg(test)]
    pub fn reset_for_test(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *inner = Inner::default();
    }
}

#[cfg(test)]
#[path = "bridge_test.rs"]
mod bridge_test;
