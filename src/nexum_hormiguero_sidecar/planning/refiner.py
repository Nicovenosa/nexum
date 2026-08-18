"""Refiner acotado (SPEC-HORMIGUERO-PLANNING-001).

Solo actúa cuando:
1. TODOS los hallazgos del Critic son corregibles (uno fatal ⇒ no refinar);
2. queda budget de iteraciones del perfil.

Arreglos permitidos (mecánicos, jamás inventan contenido):
- truncar pasos al límite del perfil (preservando prefijo);
- renombrar ids duplicados de forma determinística;
- podar dependencias huérfanas;
- agregar stop_conditions default;
- elevar risk declarado cuando hay tools (low → medium);
- clamp del budget al perfil.

Trabaja SIEMPRE sobre una copia (Frozen Snapshot afuera, en pipeline.py).
"""

from __future__ import annotations

import copy

from .critic import CriticFinding, FindingCode
from .profiles import Profile
from .schema import Plan

DEFAULT_STOP_CONDITIONS = ["budget_exhausted", "validator_rejected"]


def refine(plan: Plan, findings: list[CriticFinding], profile: Profile) -> Plan | None:
    """Devuelve el plan refinado, o None si hay hallazgos fatales (no refinar)."""
    if any(not f.correctable for f in findings):
        return None
    refined = copy.deepcopy(plan)
    codes = {f.code for f in findings}

    if FindingCode.DUP_STEP_IDS in codes:
        seen: dict[str, int] = {}
        for step in refined.steps:
            n = seen.get(step.id, 0)
            seen[step.id] = n + 1
            if n:
                step.id = f"{step.id}-{n + 1}"

    if FindingCode.TOO_MANY_STEPS in codes:
        refined.steps = refined.steps[: profile.max_steps]

    # Poda de deps huérfanas (también las que dejó el truncado).
    known = {s.id for s in refined.steps}
    for step in refined.steps:
        step.depends_on = [d for d in step.depends_on if d in known]

    if FindingCode.STOP_CONDITIONS_MISSING in codes:
        refined.stop_conditions = list(DEFAULT_STOP_CONDITIONS)

    if FindingCode.RISK_MISMATCH in codes and refined.risk == "low":
        refined.risk = "medium"

    if FindingCode.BUDGET_EXCEEDED in codes:
        refined.budget.max_steps = min(refined.budget.max_steps, profile.max_steps)
        refined.budget.max_latency_ms = min(
            refined.budget.max_latency_ms, profile.max_plan_latency_ms
        )
        refined.budget.max_refine_iterations = min(
            refined.budget.max_refine_iterations, profile.max_refine_iterations
        )

    return refined
