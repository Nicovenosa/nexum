"""Motor de Nocturno (SPEC-NOCTURNO-001): gates + promoción controlada.

El evaluador es inyectable: `evaluator(policy: dict, dataset: list[dict]) ->
{"accuracy": float, "false_local": float, "latency_ms": float}`. Nocturno no
inventa métricas: corre el MISMO evaluador sobre baseline y candidato con el
MISMO dataset reproducible, y compara.
"""

from __future__ import annotations

import json
import os
import time
from enum import Enum
from pathlib import Path
from typing import Any, Callable

from nexum_experience.evidence import EvidenceRecord, EvidenceStore

from .candidates import (
    CANDIDATE_KINDS,
    IMMUTABLE_POLICY_KEYS,
    Candidate,
    CandidateStore,
    PromotionState,
)

Evaluator = Callable[[dict[str, Any], list[dict]], dict[str, float]]


class NocturnoMode(str, Enum):
    DISABLED = "disabled"
    MANUAL = "manual"
    DRY_RUN = "dry_run"
    BENCHMARK_ONLY = "benchmark_only"
    SHADOW = "shadow"
    SCHEDULED = "scheduled"


def mode_from_env() -> NocturnoMode:
    raw = os.environ.get("NEXUM_NOCTURNO_MODE", "dry_run").strip().lower()
    try:
        return NocturnoMode(raw)
    except ValueError:
        return NocturnoMode.DRY_RUN


def autopromote_enabled() -> bool:
    """OFF por defecto (vinculante v0.1). Incluso ON, approve() es explícito."""
    return os.environ.get("NEXUM_NOCTURNO_AUTOPROMOTE") == "1"


class Nocturno:
    def __init__(
        self,
        base_dir: Path | str,
        evidence: EvidenceStore,
        evaluator: Evaluator,
        policy_defaults: dict[str, Any] | None = None,
    ) -> None:
        self.base_dir = Path(base_dir)
        self.base_dir.mkdir(parents=True, exist_ok=True)
        self.candidates = CandidateStore(self.base_dir / "candidates.db")
        self.evidence = evidence
        self.evaluator = evaluator
        self.policy_path = self.base_dir / "policy.json"
        if not self.policy_path.exists():
            self._write_policy(
                {"version": "nocturno-policy/v1", **(policy_defaults or {})}
            )

    # ── política activa ──────────────────────────────────────────────

    def active_policy(self) -> dict[str, Any]:
        return json.loads(self.policy_path.read_text(encoding="utf-8"))

    def _write_policy(self, policy: dict[str, Any]) -> None:
        tmp = self.policy_path.with_suffix(".tmp")
        tmp.write_text(json.dumps(policy, indent=2, sort_keys=True), encoding="utf-8")
        tmp.replace(self.policy_path)

    def _evidence(self, action: str, cand: Candidate, result: str, **metric) -> str:
        rec = EvidenceRecord(
            source="benchmark" if "bench" in action else "validator",
            environment="low/linux-x86_64",
            artifact=f"nocturno-candidate:{cand.candidate_id[:8]}",
            policy_version=cand.policy_version_base,
            result=result,
            confidence=0.9,
            provenance=f"nocturno:{action}",
            metric_name=metric.get("name", ""),
            metric_value=float(metric.get("value", 0.0)),
            baseline_value=float(metric.get("baseline", 0.0)),
        )
        h = self.evidence.append(rec)
        cand.evidence_ids.append(rec.evidence_id)
        return h

    # ── generación de hipótesis/candidatos ───────────────────────────

    def propose(
        self, kind: str, hypothesis_code: str, payload: dict[str, Any]
    ) -> Candidate:
        if kind not in CANDIDATE_KINDS:
            raise ValueError(f"kind desconocido: {kind}")
        cand = Candidate(
            kind=kind,
            hypothesis_code=hypothesis_code,
            payload=payload,
            policy_version_base=str(self.active_policy().get("version")),
        )
        self.candidates.save(cand)
        return cand

    def detect_patterns(self, replay: list[dict]) -> list[dict[str, Any]]:
        """Agregaciones determinísticas sobre el replay dataset (sin LLM):
        rutas lentas, escaladas dominantes, clases con fallos."""
        agg: dict[tuple[str, str], dict[str, float]] = {}
        for ev in replay:
            key = (str(ev.get("task_class")), str(ev.get("route_selected")))
            a = agg.setdefault(key, {"n": 0, "lat": 0.0, "fail": 0, "dedup": 0})
            a["n"] += 1
            a["lat"] += float(ev.get("latency_ms_total") or 0)
            a["fail"] += 1 if ev.get("outcome") == "failure" else 0
            a["dedup"] += int(ev.get("dedup_count") or 1)
        patterns = []
        for (task_class, route), a in agg.items():
            if a["n"] == 0:
                continue
            patterns.append(
                {
                    "task_class": task_class,
                    "route": route,
                    "count": a["n"],
                    "avg_latency_ms": a["lat"] / a["n"],
                    "failure_rate": a["fail"] / a["n"],
                    "repeat_volume": a["dedup"],
                }
            )
        return sorted(patterns, key=lambda p: -p["repeat_volume"])

    # ── gates (en orden, cada uno persiste estado + evidencia) ───────

    def security_gate(self, cand: Candidate) -> bool:
        """Un candidato que toque una immutable policy se rechaza SIEMPRE.
        Corre primero: ni siquiera merece sandbox."""
        touched = {k.lower() for k in cand.payload} & IMMUTABLE_POLICY_KEYS
        if touched:
            cand.reject_reason = f"immutable_policy:{'/'.join(sorted(touched))}"
            cand.transition(PromotionState.REJECTED)
            self._evidence("security_gate", cand, "fail")
            self.candidates.save(cand)
            return False
        return True

    def sandbox(self, cand: Candidate, dataset: list[dict]) -> None:
        """Replay del dataset contra baseline y candidato. Sin tocar producción."""
        baseline_policy = self.active_policy()
        candidate_policy = {**baseline_policy, **cand.payload}
        cand.baseline_metrics = self.evaluator(baseline_policy, dataset)
        cand.candidate_metrics = self.evaluator(candidate_policy, dataset)
        cand.transition(PromotionState.SANDBOXED)
        self._evidence(
            "sandbox",
            cand,
            "pass",
            name="accuracy",
            value=cand.candidate_metrics.get("accuracy", 0.0),
            baseline=cand.baseline_metrics.get("accuracy", 0.0),
        )
        self.candidates.save(cand)

    def benchmark_gate(self, cand: Candidate, min_improvement: float = 0.005) -> bool:
        """Mejora material o afuera. Neutro también se rechaza (no sumar
        complejidad sin beneficio)."""
        b, c = cand.baseline_metrics, cand.candidate_metrics
        improved_acc = c["accuracy"] > b["accuracy"] + min_improvement
        improved_lat = (
            c["latency_ms"] < b["latency_ms"] * 0.9 and c["accuracy"] >= b["accuracy"]
        )
        if improved_acc or improved_lat:
            cand.transition(PromotionState.BENCHMARKED)
            self._evidence(
                "benchmark",
                cand,
                "improved",
                name="accuracy",
                value=c["accuracy"],
                baseline=b["accuracy"],
            )
            self.candidates.save(cand)
            return True
        cand.reject_reason = "benchmark:no_material_improvement"
        cand.transition(PromotionState.REJECTED)
        self._evidence(
            "benchmark",
            cand,
            "neutral",
            name="accuracy",
            value=c["accuracy"],
            baseline=b["accuracy"],
        )
        self.candidates.save(cand)
        return False

    def regression_gate(self, cand: Candidate) -> bool:
        """false_local no puede empeorar NI UN CASO (regla dura del corpus)."""
        if cand.candidate_metrics["false_local"] > cand.baseline_metrics["false_local"]:
            cand.reject_reason = "regression:false_local_increased"
            cand.transition(PromotionState.REJECTED)
            self._evidence(
                "regression",
                cand,
                "regressed",
                name="false_local",
                value=cand.candidate_metrics["false_local"],
                baseline=cand.baseline_metrics["false_local"],
            )
            self.candidates.save(cand)
            return False
        cand.transition(PromotionState.REGRESSION_VALIDATED)
        self._evidence("regression", cand, "pass")
        self.candidates.save(cand)
        return True

    def resource_gate(self, cand: Candidate, max_latency_ms: float = 800.0) -> bool:
        if cand.candidate_metrics.get("latency_ms", 0.0) > max_latency_ms:
            cand.reject_reason = "resource:latency_budget"
            cand.transition(PromotionState.REJECTED)
            self._evidence("resource_gate", cand, "fail")
            self.candidates.save(cand)
            return False
        cand.transition(PromotionState.SECURITY_VALIDATED)
        self._evidence("resource_gate", cand, "pass")
        self.candidates.save(cand)
        return True

    def shadow(self, cand: Candidate, dataset: list[dict]) -> None:
        """Evaluación en paralelo SIN aplicar (la política activa no cambia)."""
        candidate_policy = {**self.active_policy(), **cand.payload}
        cand.shadow_metrics = self.evaluator(candidate_policy, dataset)
        cand.transition(PromotionState.SHADOW)
        self._evidence(
            "shadow",
            cand,
            "pass",
            name="accuracy",
            value=cand.shadow_metrics.get("accuracy", 0.0),
        )
        self.candidates.save(cand)

    def request_approval(self, cand: Candidate) -> None:
        cand.transition(PromotionState.APPROVAL_PENDING)
        self.candidates.save(cand)

    # ── promoción / rechazo / rollback (comandos explícitos) ─────────

    def approve(self, candidate_id: str) -> Candidate:
        """Promoción por comando explícito. Frozen Snapshot + version bump +
        rollback pointer + evidencia. AUTOPROMOTE no existe como camino."""
        cand = self.candidates.load(candidate_id)
        if cand is None:
            raise KeyError(candidate_id)
        current = self.active_policy()
        cand.frozen_snapshot = dict(current)
        cand.rollback_pointer = str(current.get("version"))
        new_policy = {**current, **cand.payload}
        base, _, rev = str(current.get("version", "v1")).rpartition("/")
        new_rev = f"v{int(rev.lstrip('v') or 1) + 1}" if rev else "v2"
        new_policy["version"] = f"{base or 'nocturno-policy'}/{new_rev}"
        new_policy["changelog"] = (
            f"candidate:{cand.candidate_id[:8]} kind:{cand.kind} "
            f"hypothesis:{cand.hypothesis_code}"
        )
        cand.transition(PromotionState.PROMOTED)
        self._write_policy(new_policy)
        self._evidence("promotion", cand, "pass")
        self.candidates.save(cand)
        return cand

    def reject(self, candidate_id: str, reason: str) -> Candidate:
        cand = self.candidates.load(candidate_id)
        if cand is None:
            raise KeyError(candidate_id)
        cand.reject_reason = reason
        cand.transition(PromotionState.REJECTED)
        self._evidence("manual_reject", cand, "fail")
        self.candidates.save(cand)
        return cand

    def rollback(self, candidate_id: str) -> Candidate:
        """Restaura EXACTAMENTE el Frozen Snapshot previo a la promoción."""
        cand = self.candidates.load(candidate_id)
        if cand is None:
            raise KeyError(candidate_id)
        if not cand.frozen_snapshot:
            raise RuntimeError("candidato sin snapshot: no fue promovido")
        cand.transition(PromotionState.ROLLED_BACK)
        self._write_policy(dict(cand.frozen_snapshot))
        self._evidence("rollback", cand, "pass")
        self.candidates.save(cand)
        return cand

    # ── ciclo completo (hasta approval pending; promover es humano) ──

    def run_cycle(self, cand: Candidate, dataset: list[dict]) -> Candidate:
        """PROPOSED → … → APPROVAL_PENDING o REJECTED. Jamás promueve solo."""
        t0 = time.monotonic()
        if not self.security_gate(cand):
            return cand
        self.sandbox(cand, dataset)
        if not self.benchmark_gate(cand):
            return cand
        if not self.regression_gate(cand):
            return cand
        if not self.resource_gate(cand):
            return cand
        self.shadow(cand, dataset)
        self.request_approval(cand)
        _ = t0
        return cand
