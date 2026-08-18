//! Workers (Fase A) — dispatch TIPADO y ACOTADO de pasos del plan bajo autoridad
//! exclusiva de Rust. Los workers:
//!   - usan contratos tipados (id, capabilities, scopes, timeout, output_keys);
//!   - NO se conceden permisos (Rust autoriza cada dispatch);
//!   - NO seleccionan providers ni ejecutan tools fuera de Rust;
//!   - soportan timeout y cancelación;
//!   - retornan resultados/errores tipados;
//!   - registran resultados verificables (evidencia).
//!
//! Rust conserva routing, permisos, HITL, riesgo, providers, tools y ejecución
//! final. Python nunca es segundo control plane: este dispatch es 100% Rust.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::cartero::{StepContext, MAX_CONTEXT_BYTES};

/// Errores tipados del contrato de worker (vista pública, sin datos crudos).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerError {
    Unknown,
    UnauthorizedCapability,
    Timeout,
    Cancelled,
    MalformedResult,
    ContextTooLarge,
    ScopeLeak,
    SecretLeak,
    DoubleDispatch,
}

impl WorkerError {
    pub fn code(&self) -> &'static str {
        match self {
            WorkerError::Unknown => "unknown_worker",
            WorkerError::UnauthorizedCapability => "unauthorized_capability",
            WorkerError::Timeout => "timeout",
            WorkerError::Cancelled => "cancelled",
            WorkerError::MalformedResult => "malformed_result",
            WorkerError::ContextTooLarge => "context_too_large",
            WorkerError::ScopeLeak => "scope_leak",
            WorkerError::SecretLeak => "secret_leak",
            WorkerError::DoubleDispatch => "double_dispatch",
        }
    }
}

pub type WorkerHandler =
    Arc<dyn Fn(&WorkerRequest) -> Result<BTreeMap<String, String>, WorkerError> + Send + Sync>;

/// Contrato tipado de un worker.
pub struct WorkerContract {
    pub worker_id: String,
    pub capabilities: Vec<String>,
    pub allowed_scopes: Vec<String>,
    pub timeout: Duration,
    pub requires_approval: bool,
    /// Claves que el output DEBE contener (si faltan ⇒ MalformedResult).
    pub output_keys: Vec<String>,
    pub handler: WorkerHandler,
}

/// Pedido tipado a un worker.
pub struct WorkerRequest {
    pub context: StepContext,
    pub cancel: Arc<AtomicBool>,
}

/// Resultado tipado.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerOutcome {
    pub ok: bool,
    pub worker_id: String,
    pub capability: String,
    pub output: BTreeMap<String, String>,
    pub latency_ms: u128,
    pub required_approval: bool,
}

/// Registry de workers + guards de despacho (doble dispatch, nunca-despachados).
pub struct WorkerRegistry {
    contracts: HashMap<String, WorkerContract>,
    /// worker_ids "activos" (deben despacharse al menos una vez).
    active: HashSet<String>,
    /// request_ids ya despachados (guard de doble dispatch).
    dispatched_requests: Mutex<HashSet<String>>,
    /// worker_ids efectivamente despachados (para el gate never-dispatched).
    dispatched_workers: Mutex<HashSet<String>>,
}

impl Default for WorkerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerRegistry {
    pub fn new() -> Self {
        Self {
            contracts: HashMap::new(),
            active: HashSet::new(),
            dispatched_requests: Mutex::new(HashSet::new()),
            dispatched_workers: Mutex::new(HashSet::new()),
        }
    }

    /// Registra un contrato. `active=true` ⇒ debe despacharse (gate never-dispatched).
    pub fn register(&mut self, contract: WorkerContract, active: bool) {
        if active {
            self.active.insert(contract.worker_id.clone());
        }
        self.contracts.insert(contract.worker_id.clone(), contract);
    }

    pub fn is_registered(&self, worker_id: &str) -> bool {
        self.contracts.contains_key(worker_id)
    }

    /// Workers activos registrados que NUNCA se despacharon (gate = 0).
    pub fn active_never_dispatched(&self) -> Vec<String> {
        let done = self.dispatched_workers.lock().unwrap_or_else(|e| e.into_inner());
        self.active.iter().filter(|w| !done.contains(*w)).cloned().collect()
    }

    /// Despacha un paso a un worker BAJO AUTORIDAD DE RUST.
    ///
    /// `authorized` = decisión de autorización de Rust (risk/HITL). El worker
    /// jamás se autoriza a sí mismo. `request_id` identifica el despacho único.
    pub fn dispatch(
        &self,
        worker_id: &str,
        capability: &str,
        request_id: &str,
        context: StepContext,
        cancel: Arc<AtomicBool>,
        authorized: bool,
    ) -> Result<WorkerOutcome, WorkerError> {
        // 1. worker conocido
        let contract = self.contracts.get(worker_id).ok_or(WorkerError::Unknown)?;

        // 2. capability declarada en el contrato
        if !contract.capabilities.iter().any(|c| c == capability) {
            return Err(WorkerError::UnauthorizedCapability);
        }

        // 3. autorización de Rust (el worker NO se concede permisos)
        if !authorized {
            return Err(WorkerError::UnauthorizedCapability);
        }

        // 4. scope leak: el scope del contexto debe ser subconjunto del permitido
        if !context.scope.iter().all(|s| contract.allowed_scopes.iter().any(|a| a == s)) {
            return Err(WorkerError::ScopeLeak);
        }

        // 5. secret leak: el payload debe venir ya redactado (defensa en profundidad)
        if crate::ui::secret_redact::redact_secrets(&context.payload) != context.payload {
            return Err(WorkerError::SecretLeak);
        }

        // 6. unbounded context
        if context.size_bytes > MAX_CONTEXT_BYTES {
            return Err(WorkerError::ContextTooLarge);
        }

        // 7. double dispatch guard (idempotencia por request_id)
        {
            let mut seen = self.dispatched_requests.lock().unwrap_or_else(|e| e.into_inner());
            if !seen.insert(request_id.to_string()) {
                return Err(WorkerError::DoubleDispatch);
            }
        }

        // 8. cancelación previa
        if cancel.load(Ordering::Relaxed) {
            return Err(WorkerError::Cancelled);
        }

        // 9. ejecución con timeout duro (handler en thread detached; deadline por recv_timeout)
        let started = Instant::now();
        let req = WorkerRequest { context: context.clone(), cancel: cancel.clone() };
        let output = run_with_timeout(contract.handler.clone(), req, contract.timeout, &cancel)?;

        // 10. validación de output (claves garantizadas)
        if !contract.output_keys.iter().all(|k| output.contains_key(k)) {
            return Err(WorkerError::MalformedResult);
        }

        // marcar worker como despachado (gate never-dispatched)
        self.dispatched_workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(worker_id.to_string());

        Ok(WorkerOutcome {
            ok: true,
            worker_id: worker_id.to_string(),
            capability: capability.to_string(),
            output,
            latency_ms: started.elapsed().as_millis(),
            required_approval: contract.requires_approval,
        })
    }
}

use std::sync::OnceLock;

static REGISTRY: OnceLock<WorkerRegistry> = OnceLock::new();

/// Registry singleton del proceso con los workers por defecto del producto.
pub fn registry() -> &'static WorkerRegistry {
    REGISTRY.get_or_init(build_default_registry)
}

/// Workers por defecto. `plan_step` gobierna pasos de SOLO LECTURA del plan bajo
/// autoridad de Rust: construye evidencia de que el paso fue contextualizado y
/// autorizado, SIN ejecutar tools (eso lo hace el agente con HITL). Los pasos de
/// escritura/exec/red se difieren a HITL, nunca se auto-ejecutan.
fn build_default_registry() -> WorkerRegistry {
    let mut reg = WorkerRegistry::new();
    let handler: WorkerHandler = Arc::new(|req: &WorkerRequest| {
        let mut o = BTreeMap::new();
        o.insert("status".to_string(), "authorized".to_string());
        o.insert("capability".to_string(), req.context.capability.clone());
        o.insert("scope".to_string(), req.context.scope.join(","));
        Ok(o)
    });
    reg.register(
        WorkerContract {
            worker_id: "plan_step".into(),
            capabilities: vec!["read".into()],
            allowed_scopes: vec!["fs:read".into(), "ctx:read".into()],
            timeout: Duration::from_millis(500),
            requires_approval: false,
            output_keys: vec!["status".into()],
            handler,
        },
        true,
    );
    reg
}

/// Ejecuta el handler con timeout duro y respeto de cancelación. El handler
/// corre en un thread DETACHED: si cuelga, `recv_timeout` retorna Timeout sin
/// bloquear (el thread queda huérfano hasta que el handler termine — red de
/// seguridad, no el caso normal; los workers son acotados).
fn run_with_timeout(
    handler: WorkerHandler,
    req: WorkerRequest,
    timeout: Duration,
    cancel: &Arc<AtomicBool>,
) -> Result<BTreeMap<String, String>, WorkerError> {
    let (tx, rx) = mpsc::channel();
    let cancel_thread = cancel.clone();
    std::thread::spawn(move || {
        let r = if cancel_thread.load(Ordering::Relaxed) {
            Err(WorkerError::Cancelled)
        } else {
            handler(&req)
        };
        let _ = tx.send(r);
    });
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(WorkerError::Timeout),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(WorkerError::MalformedResult),
    }
}

#[cfg(test)]
#[path = "workers_test.rs"]
mod workers_test;
