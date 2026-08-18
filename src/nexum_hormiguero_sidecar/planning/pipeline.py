"""Pipeline Planner → Critic → Refiner → Validator (SPEC-HORMIGUERO-PLANNING-001).

Frozen Snapshot: el borrador original se congela antes de refinar; si el
refinado no valida, el resultado es rechazo con el snapshot intacto (rollback
exacto — jamás se entrega un plan a medio refinar).
"""

from __future__ import annotations

import copy
from dataclasses import dataclass, field
from typing import Any

from .critic import CriticFinding, criticize
from .planner import draft_plan
from .profiles import Profile
from .refiner import refine
from .routes import POLICY_VERSION, ReasonCode
from .schema import Plan, ValidationError
from .validator import validate


@dataclass
class PlanOutcome:
    status: str  # "validated" | "refined_validated" | "rejected" | "needs_user_input"
    reason_code: str
    policy_version: str = POLICY_VERSION
    plan: Plan | None = None
    frozen_snapshot: Plan | None = None
    findings: list[CriticFinding] = field(default_factory=list)
    errors: list[ValidationError] = field(default_factory=list)
    refine_iterations: int = 0

    def public_dict(self) -> dict[str, Any]:
        """Vista segura: decisión + códigos, jamás reasoning interno."""
        return {
            "status": self.status,
            "reason_code": self.reason_code,
            "policy_version": self.policy_version,
            "plan": self.plan.to_dict() if self.plan else None,
            "findings": [f.code.value for f in self.findings],
            "errors": [e.value for e in self.errors],
            "refine_iterations": self.refine_iterations,
        }


def run_pipeline(text: str, task_class: str, profile: Profile) -> PlanOutcome:
    """Ejecuta el pipeline completo sobre un pedido. Determinístico y total."""
    draft = draft_plan(text, task_class, profile)
    if draft is None:
        return PlanOutcome(
            status="needs_user_input",
            reason_code=ReasonCode.NEEDS_USER_INPUT.value,
        )
    return run_pipeline_on_plan(draft, profile)


def run_pipeline_on_plan(draft: Plan, profile: Profile) -> PlanOutcome:
    """Pipeline sobre un plan ya construido (para fixtures/tests/futuro LLM)."""
    frozen = copy.deepcopy(draft)  # Frozen Snapshot: rollback exacto
    findings = criticize(draft, profile)

    if not findings:
        plan, result = validate(draft, profile)
        if result.ok:
            return PlanOutcome(
                status="validated",
                reason_code=ReasonCode.PLAN_VALIDATED.value,
                plan=plan,
                frozen_snapshot=frozen,
            )
        return PlanOutcome(
            status="rejected",
            reason_code=ReasonCode.PLAN_REJECTED.value,
            frozen_snapshot=frozen,
            errors=result.errors,
        )

    # Hallazgos fatales ⇒ rechazo directo, sin refinar (no adivinar).
    if any(not f.correctable for f in findings):
        return PlanOutcome(
            status="rejected",
            reason_code=ReasonCode.PLAN_REJECTED.value,
            frozen_snapshot=frozen,
            findings=findings,
        )

    # Refine acotado por el perfil.
    iterations = 0
    current = draft
    while iterations < profile.max_refine_iterations:
        refined = refine(current, findings, profile)
        if refined is None:
            break
        iterations += 1
        current = refined
        findings = criticize(current, profile)
        if not findings:
            break

    plan, result = validate(current, profile)
    if result.ok and not findings:
        return PlanOutcome(
            status="refined_validated",
            reason_code=ReasonCode.PLAN_REFINED.value,
            plan=plan,
            frozen_snapshot=frozen,
            refine_iterations=iterations,
        )
    return PlanOutcome(
        status="rejected",
        reason_code=ReasonCode.PLAN_REJECTED.value,
        frozen_snapshot=frozen,
        findings=findings,
        errors=result.errors,
        refine_iterations=iterations,
    )
