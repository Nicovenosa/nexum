"""Validator determinístico fail-closed (SPEC-HORMIGUERO-PLANNING-001).

El Router SOLO recibe planes que pasaron por acá. Nada de LLM: reglas puras.
Rechaza: schema inválido, pasos > límite, ciclos, deps desconocidas, tool
fuera de allowlist, tools prohibidas (approval bypass), claims sin evidencia,
ruta prohibida, budget roto, stop_conditions ausentes, risk mismatch.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from .profiles import Profile
from .routes import PLAN_ALLOWED_ROUTES, RouteClass
from .schema import RISKS, STEP_KINDS, TASK_CLASSES, Plan, ValidationError, parse_plan

# Allowlist de tools que un plan puede DECLARAR como intención. La ejecución y
# la aprobación viven en el runtime (Security/Tools) — esto solo acota lo
# declarable. Nunca se amplía desde un LLM.
DEFAULT_TOOL_ALLOWLIST = frozenset(
    {"read_file", "list_dir", "search", "memory_recall", "memory_save_proposal"}
)

# Tools que un plan JAMÁS puede declarar (bypass de aprobación/seguridad):
# ejecución, borrado, escritura directa, red arbitraria, secretos.
FORBIDDEN_TOOLS = frozenset(
    {
        "shell",
        "bash",
        "exec",
        "delete",
        "rm",
        "write_file_unapproved",
        "network",
        "secrets",
        "approve",
        "sudo",
    }
)


@dataclass
class ValidationResult:
    ok: bool
    errors: list[ValidationError] = field(default_factory=list)


def validate(
    raw_plan: Any,
    profile: Profile,
    tool_allowlist: frozenset[str] = DEFAULT_TOOL_ALLOWLIST,
) -> tuple[Plan | None, ValidationResult]:
    """Valida un plan crudo. Devuelve (plan_parseado, resultado). Fail-closed:
    ante cualquier duda el plan NO pasa.
    """
    errors: list[ValidationError] = []
    plan = raw_plan if isinstance(raw_plan, Plan) else parse_plan(raw_plan)
    if plan is None:
        return None, ValidationResult(ok=False, errors=[ValidationError.SCHEMA_INVALID])

    if plan.task_class not in TASK_CLASSES or plan.risk not in RISKS:
        errors.append(ValidationError.SCHEMA_INVALID)

    # Ruta: solo las permitidas para planes, y jamás DENY/ASK/PASSTHROUGH.
    try:
        route = RouteClass(plan.route)
    except ValueError:
        route = None
    if route is None or route not in PLAN_ALLOWED_ROUTES:
        errors.append(ValidationError.ROUTE_FORBIDDEN)

    # Pasos: presencia y límite del perfil (el budget del plan no puede
    # exceder el del perfil — un LLM no negocia budgets).
    if not plan.steps:
        errors.append(ValidationError.NO_STEPS)
    if len(plan.steps) > profile.max_steps:
        errors.append(ValidationError.TOO_MANY_STEPS)
    if (
        plan.budget.max_steps > profile.max_steps
        or plan.budget.max_refine_iterations > profile.max_refine_iterations
        or plan.budget.max_latency_ms > profile.max_plan_latency_ms
        or plan.budget.max_steps <= 0
        or plan.budget.max_latency_ms <= 0
        or plan.budget.max_refine_iterations < 0
    ):
        errors.append(ValidationError.BUDGET_INVALID)

    # IDs únicos + kinds conocidos.
    ids = [s.id for s in plan.steps]
    if len(ids) != len(set(ids)):
        errors.append(ValidationError.SCHEMA_INVALID)
    if any(s.kind not in STEP_KINDS for s in plan.steps):
        errors.append(ValidationError.SCHEMA_INVALID)

    # Dependencias: conocidas y sin ciclos (DFS).
    known = set(ids)
    if any(d not in known for s in plan.steps for d in s.depends_on):
        errors.append(ValidationError.DEP_UNKNOWN)
    elif _has_cycle(plan):
        errors.append(ValidationError.DEP_CYCLE)

    # Tools: prohibidas = bypass de aprobación (fatal); fuera de allowlist =
    # desconocida. Vale para steps y para tool_intents declarados.
    declared = {s.tool for s in plan.steps if s.tool} | set(plan.tool_intents)
    if declared & FORBIDDEN_TOOLS:
        errors.append(ValidationError.APPROVAL_BYPASS)
    if any(t not in tool_allowlist and t not in FORBIDDEN_TOOLS for t in declared):
        errors.append(ValidationError.TOOL_UNKNOWN)

    # Evidencia: pasos verify/report deben referenciar evidencia y el plan
    # debe declarar expected_evidence (sin evidencia no hay claims).
    needs_evidence = [s for s in plan.steps if s.kind in ("verify", "report")]
    if needs_evidence and (
        not plan.expected_evidence or any(not s.evidence for s in needs_evidence)
    ):
        errors.append(ValidationError.EVIDENCE_MISSING)

    # Stop conditions: obligatorias (loops acotados por contrato).
    if not plan.stop_conditions:
        errors.append(ValidationError.STOP_CONDITIONS_MISSING)

    # Risk mismatch: declarar tools con risk=low es inconsistente (las tools
    # implican al menos medium — la aprobación real la decide Security).
    if declared and plan.risk == "low":
        errors.append(ValidationError.RISK_MISMATCH)

    return plan, ValidationResult(ok=not errors, errors=errors)


def _has_cycle(plan: Plan) -> bool:
    graph = {s.id: list(s.depends_on) for s in plan.steps}
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

    return any(color[n] == WHITE and dfs(n) for n in graph)
