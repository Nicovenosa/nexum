"""Critic determinístico (SPEC-HORMIGUERO-PLANNING-001).

Detecta fallas ANTES del Validator y las clasifica en corregibles (el Refiner
puede arreglarlas dentro del budget) o fatales (rechazo directo). El Critic no
inventa contenido: solo señala.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum

from .profiles import Profile
from .schema import STEP_KINDS, Plan
from .validator import DEFAULT_TOOL_ALLOWLIST, FORBIDDEN_TOOLS


class FindingCode(str, Enum):
    NO_STEPS = "no_steps"  # fatal: no hay nada que refinar
    TOO_MANY_STEPS = "too_many_steps"  # corregible: truncar al límite
    DUP_STEP_IDS = "dup_step_ids"  # corregible: renombrar duplicados
    DEP_UNKNOWN = "dep_unknown"  # corregible: podar deps huérfanas
    DEP_CYCLE = "dep_cycle"  # fatal: el orden es ambiguo, no adivinar
    UNKNOWN_KIND = "unknown_kind"  # fatal: contrato roto
    TOOL_FORBIDDEN = "tool_forbidden"  # fatal: bypass de aprobación
    TOOL_UNKNOWN = "tool_unknown"  # fatal: no inventar capacidades
    EVIDENCE_MISSING = "evidence_missing"  # fatal: claims sin evidencia
    STOP_CONDITIONS_MISSING = "stop_conditions_missing"  # corregible: default
    RISK_MISMATCH = "risk_mismatch"  # corregible: elevar risk declarado
    BUDGET_EXCEEDED = "budget_exceeded"  # corregible: clamp al perfil


CORRECTABLE = frozenset(
    {
        FindingCode.TOO_MANY_STEPS,
        FindingCode.DUP_STEP_IDS,
        FindingCode.DEP_UNKNOWN,
        FindingCode.STOP_CONDITIONS_MISSING,
        FindingCode.RISK_MISMATCH,
        FindingCode.BUDGET_EXCEEDED,
    }
)


@dataclass
class CriticFinding:
    code: FindingCode
    detail: str
    correctable: bool


def criticize(plan: Plan, profile: Profile) -> list[CriticFinding]:
    findings: list[CriticFinding] = []

    def add(code: FindingCode, detail: str) -> None:
        findings.append(
            CriticFinding(code=code, detail=detail, correctable=code in CORRECTABLE)
        )

    if not plan.steps:
        add(FindingCode.NO_STEPS, "el plan no tiene pasos")
        return findings

    if len(plan.steps) > profile.max_steps:
        add(
            FindingCode.TOO_MANY_STEPS,
            f"{len(plan.steps)} pasos > límite {profile.max_steps} del perfil {profile.name}",
        )

    ids = [s.id for s in plan.steps]
    if len(ids) != len(set(ids)):
        add(FindingCode.DUP_STEP_IDS, "ids de paso duplicados")

    known = set(ids)
    orphan = [d for s in plan.steps for d in s.depends_on if d not in known]
    if orphan:
        add(
            FindingCode.DEP_UNKNOWN, f"dependencias desconocidas: {sorted(set(orphan))}"
        )

    if _has_cycle_ids(plan):
        add(FindingCode.DEP_CYCLE, "ciclo de dependencias")

    bad_kinds = sorted({s.kind for s in plan.steps if s.kind not in STEP_KINDS})
    if bad_kinds:
        add(FindingCode.UNKNOWN_KIND, f"kinds desconocidos: {bad_kinds}")

    declared = {s.tool for s in plan.steps if s.tool} | set(plan.tool_intents)
    forbidden = sorted(declared & FORBIDDEN_TOOLS)
    if forbidden:
        add(
            FindingCode.TOOL_FORBIDDEN,
            f"tools prohibidas (approval bypass): {forbidden}",
        )
    unknown = sorted(
        t
        for t in declared
        if t not in DEFAULT_TOOL_ALLOWLIST and t not in FORBIDDEN_TOOLS
    )
    if unknown:
        add(FindingCode.TOOL_UNKNOWN, f"tools fuera de allowlist: {unknown}")

    needs_ev = [s for s in plan.steps if s.kind in ("verify", "report")]
    if needs_ev and (
        not plan.expected_evidence or any(not s.evidence for s in needs_ev)
    ):
        add(FindingCode.EVIDENCE_MISSING, "pasos verify/report sin evidencia declarada")

    if not plan.stop_conditions:
        add(FindingCode.STOP_CONDITIONS_MISSING, "sin stop_conditions")

    if declared and plan.risk == "low":
        add(FindingCode.RISK_MISMATCH, "declara tools con risk=low")

    if (
        plan.budget.max_steps > profile.max_steps
        or plan.budget.max_latency_ms > profile.max_plan_latency_ms
        or plan.budget.max_refine_iterations > profile.max_refine_iterations
    ):
        add(FindingCode.BUDGET_EXCEEDED, "budget del plan excede el perfil")

    return findings


def _has_cycle_ids(plan: Plan) -> bool:
    graph = {s.id: [d for d in s.depends_on] for s in plan.steps}
    WHITE, GRAY, BLACK = 0, 1, 2
    color = dict.fromkeys(graph, WHITE)

    def dfs(node: str) -> bool:
        color[node] = GRAY
        for dep in graph.get(node, []):
            if color.get(dep) == GRAY:
                return True
            if color.get(dep) == WHITE and dfs(dep):
                return True
        color[node] = BLACK
        return False

    return any(color.get(n) == WHITE and dfs(n) for n in graph)
