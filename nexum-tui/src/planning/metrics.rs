//! Métricas del PlanningGateway (solo números, jamás texto de usuario).
//! Gates de la remediación:
//!   planning_generated > 0 en corpus planificable
//!   planning_consumed = planning_generated - planning_rejected
//!   ignored_valid_plans = 0 · double_routing = 0

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct PlanningMetrics {
    pub planning_requested: AtomicU64,
    pub planning_generated: AtomicU64,
    pub planning_rejected: AtomicU64,
    pub planning_consumed: AtomicU64,
    pub planning_bypassed: AtomicU64,
    pub validator_failed: AtomicU64,
    pub plan_execution_completed: AtomicU64,
    pub plan_execution_failed: AtomicU64,
    /// Invariante de auditoría: planes válidos que se ignoraron (debe ser 0).
    pub ignored_valid_plans: AtomicU64,
}

/// Snapshot inmutable (para /planning status, tests y evidencia).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlanningCounters {
    pub planning_requested: u64,
    pub planning_generated: u64,
    pub planning_rejected: u64,
    pub planning_consumed: u64,
    pub planning_bypassed: u64,
    pub validator_failed: u64,
    pub plan_execution_completed: u64,
    pub plan_execution_failed: u64,
    pub ignored_valid_plans: u64,
}

impl PlanningMetrics {
    pub fn snapshot(&self) -> PlanningCounters {
        PlanningCounters {
            planning_requested: self.planning_requested.load(Ordering::Relaxed),
            planning_generated: self.planning_generated.load(Ordering::Relaxed),
            planning_rejected: self.planning_rejected.load(Ordering::Relaxed),
            planning_consumed: self.planning_consumed.load(Ordering::Relaxed),
            planning_bypassed: self.planning_bypassed.load(Ordering::Relaxed),
            validator_failed: self.validator_failed.load(Ordering::Relaxed),
            plan_execution_completed: self.plan_execution_completed.load(Ordering::Relaxed),
            plan_execution_failed: self.plan_execution_failed.load(Ordering::Relaxed),
            ignored_valid_plans: self.ignored_valid_plans.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn inc(field: &AtomicU64) {
        field.fetch_add(1, Ordering::Relaxed);
    }
}

impl PlanningCounters {
    /// Invariante contractual: consumed == generated - rejected (todo plan válido
    /// se consume; ningún plan válido se ignora).
    pub fn invariant_holds(&self) -> bool {
        self.planning_consumed == self.planning_generated.saturating_sub(self.planning_rejected)
            && self.ignored_valid_plans == 0
    }
}
