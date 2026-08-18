"""Plan JSON tipado v1 (SPEC-HORMIGUERO-PLANNING-001).

Validación estricta hand-rolled (stdlib): tipos exactos, campos obligatorios,
sin campos extra silenciosos. Todo error es un ValidationError tipado.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any

SCHEMA_VERSION = 1

STEP_KINDS = frozenset(
    {"analyze", "read", "transform", "verify", "report", "tool_intent"}
)
TASK_CLASSES = frozenset({"summarize", "analyze", "generic", "tool", "memory"})
RISKS = frozenset({"low", "medium", "high"})


class ValidationError(str, Enum):
    """Error Enum del contrato: el Validator solo habla en estos códigos."""

    SCHEMA_INVALID = "schema_invalid"
    TOO_MANY_STEPS = "too_many_steps"
    NO_STEPS = "no_steps"
    DEP_UNKNOWN = "dep_unknown"
    DEP_CYCLE = "dep_cycle"
    TOOL_UNKNOWN = "tool_unknown"
    TOOL_FORBIDDEN = "tool_forbidden"
    APPROVAL_BYPASS = "approval_bypass"
    EVIDENCE_MISSING = "evidence_missing"
    ROUTE_FORBIDDEN = "route_forbidden"
    BUDGET_INVALID = "budget_invalid"
    STOP_CONDITIONS_MISSING = "stop_conditions_missing"
    RISK_MISMATCH = "risk_mismatch"
    INPUT_TOO_LARGE = "input_too_large"


@dataclass
class PlanStep:
    id: str
    action: str
    kind: str  # STEP_KINDS
    depends_on: list[str] = field(default_factory=list)
    evidence: list[str] = field(default_factory=list)
    tool: str | None = None


@dataclass
class Budget:
    max_steps: int
    max_latency_ms: int
    max_refine_iterations: int


@dataclass
class Plan:
    schema_version: int
    task_class: str  # TASK_CLASSES
    route: str  # RouteClass.value
    assumptions: list[str]
    steps: list[PlanStep]
    expected_evidence: list[str]
    risk: str  # RISKS
    tool_intents: list[str]
    worker_hints: list[str]
    budget: Budget
    stop_conditions: list[str]

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "task_class": self.task_class,
            "route": self.route,
            "assumptions": list(self.assumptions),
            "steps": [
                {
                    "id": s.id,
                    "action": s.action,
                    "kind": s.kind,
                    "depends_on": list(s.depends_on),
                    "evidence": list(s.evidence),
                    "tool": s.tool,
                }
                for s in self.steps
            ],
            "expected_evidence": list(self.expected_evidence),
            "risk": self.risk,
            "tool_intents": list(self.tool_intents),
            "worker_hints": list(self.worker_hints),
            "budget": {
                "max_steps": self.budget.max_steps,
                "max_latency_ms": self.budget.max_latency_ms,
                "max_refine_iterations": self.budget.max_refine_iterations,
            },
            "stop_conditions": list(self.stop_conditions),
        }


def _is_str_list(v: Any) -> bool:
    return isinstance(v, list) and all(isinstance(x, str) for x in v)


def parse_plan(raw: Any) -> Plan | None:
    """Parseo estricto: cualquier desviación de tipo/campo ⇒ None (fail-closed).

    El Validator agrega el resto de las reglas; acá solo forma y tipos.
    """
    if not isinstance(raw, dict):
        return None
    try:
        if raw.get("schema_version") != SCHEMA_VERSION:
            return None
        task_class = raw["task_class"]
        route = raw["route"]
        assumptions = raw["assumptions"]
        raw_steps = raw["steps"]
        expected_evidence = raw["expected_evidence"]
        risk = raw["risk"]
        tool_intents = raw["tool_intents"]
        worker_hints = raw["worker_hints"]
        raw_budget = raw["budget"]
        stop_conditions = raw["stop_conditions"]
    except (KeyError, TypeError):
        return None
    if not isinstance(task_class, str) or not isinstance(route, str):
        return None
    if not isinstance(risk, str):
        return None
    for lst in (
        assumptions,
        expected_evidence,
        tool_intents,
        worker_hints,
        stop_conditions,
    ):
        if not _is_str_list(lst):
            return None
    if not isinstance(raw_steps, list) or not isinstance(raw_budget, dict):
        return None
    steps: list[PlanStep] = []
    for s in raw_steps:
        if not isinstance(s, dict):
            return None
        sid = s.get("id")
        action = s.get("action")
        kind = s.get("kind")
        deps = s.get("depends_on", [])
        ev = s.get("evidence", [])
        tool = s.get("tool")
        if (
            not isinstance(sid, str)
            or not isinstance(action, str)
            or not isinstance(kind, str)
        ):
            return None
        if not _is_str_list(deps) or not _is_str_list(ev):
            return None
        if tool is not None and not isinstance(tool, str):
            return None
        steps.append(
            PlanStep(
                id=sid,
                action=action,
                kind=kind,
                depends_on=deps,
                evidence=ev,
                tool=tool,
            )
        )
    try:
        budget = Budget(
            max_steps=int(raw_budget["max_steps"]),
            max_latency_ms=int(raw_budget["max_latency_ms"]),
            max_refine_iterations=int(raw_budget["max_refine_iterations"]),
        )
    except (KeyError, TypeError, ValueError):
        return None
    return Plan(
        schema_version=SCHEMA_VERSION,
        task_class=task_class,
        route=route,
        assumptions=assumptions,
        steps=steps,
        expected_evidence=expected_evidence,
        risk=risk,
        tool_intents=tool_intents,
        worker_hints=worker_hints,
        budget=budget,
        stop_conditions=stop_conditions,
    )
