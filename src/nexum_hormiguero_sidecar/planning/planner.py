"""Planner (SPEC-HORMIGUERO-PLANNING-001).

Backend default: TEMPLATE determinístico — genera un borrador acotado según
task_class, sin LLM, sin red, reproducible. Backend LLM (qwen local vía
Ollama) queda OPT-IN con NEXUM_HORMIGUERO_PLANNER_LLM=1 y SIEMPRE cae al
template ante cualquier fallo/timeout (fail-safe).

El planner NUNCA ejecuta tools, NUNCA decide seguridad, NUNCA autoaprueba.
Produce un borrador; la autoridad es el Validator.
"""

from __future__ import annotations

import os

from .profiles import Profile
from .routes import RouteClass
from .schema import SCHEMA_VERSION, Budget, Plan, PlanStep

# task_class → (ruta, pasos template). Los templates declaran evidencia en los
# pasos verify/report (contrato: sin evidencia no hay claims).
_TEMPLATES: dict[str, tuple[RouteClass, list[tuple[str, str, list[str]]]]] = {
    "summarize": (
        RouteClass.LOCAL_PLAN,
        [
            ("s1", "analyze", []),
            ("s2", "read", ["s1"]),
            ("s3", "transform", ["s2"]),
            ("s4", "verify", ["s3"]),
            ("s5", "report", ["s4"]),
        ],
    ),
    "analyze": (
        RouteClass.PREMIUM_REASONING,
        [
            ("s1", "analyze", []),
            ("s2", "read", ["s1"]),
            ("s3", "analyze", ["s2"]),
            ("s4", "verify", ["s3"]),
            ("s5", "report", ["s4"]),
        ],
    ),
    "tool": (
        RouteClass.TOOL_INTENT,
        [
            ("s1", "analyze", []),
            ("s2", "tool_intent", ["s1"]),
            ("s3", "verify", ["s2"]),
            ("s4", "report", ["s3"]),
        ],
    ),
    "memory": (
        RouteClass.MEMORY_INTENT,
        [
            ("s1", "analyze", []),
            ("s2", "tool_intent", ["s1"]),
            ("s3", "report", ["s2"]),
        ],
    ),
    "generic": (
        RouteClass.PREMIUM_REASONING,
        [
            ("s1", "analyze", []),
            ("s2", "transform", ["s1"]),
            ("s3", "verify", ["s2"]),
            ("s4", "report", ["s3"]),
        ],
    ),
}

_KIND_ACTIONS = {
    "analyze": "entender el pedido y acotar el alcance",
    "read": "leer el contexto permitido por el Cartero",
    "transform": "elaborar el resultado dentro del alcance",
    "verify": "verificar el resultado contra la evidencia declarada",
    "report": "reportar con referencias a la evidencia",
    "tool_intent": "declarar la intención de tool (la aprueba Security, no el plan)",
}

_TOOL_BY_CLASS = {
    "tool": ["read_file"],
    "memory": ["memory_recall"],
}


def planner_llm_enabled() -> bool:
    """Backend LLM del planner: OPT-IN explícito, jamás default."""
    return os.environ.get("NEXUM_HORMIGUERO_PLANNER_LLM") == "1"


def draft_plan(text: str, task_class: str, profile: Profile) -> Plan | None:
    """Borrador determinístico por template. None si el input viola límites
    duros (input demasiado grande) — el caller decide escalar/ASK_USER.
    """
    if len(text) > profile.max_input_chars:
        return None
    template = _TEMPLATES.get(task_class)
    if template is None:
        template = _TEMPLATES["generic"]
        task_class = "generic"
    route, steps_spec = template
    evidence_ref = "resultado-observado:turno-actual"
    steps = [
        PlanStep(
            id=sid,
            action=_KIND_ACTIONS[kind],
            kind=kind,
            depends_on=list(deps),
            evidence=[evidence_ref] if kind in ("verify", "report") else [],
            tool=(
                _TOOL_BY_CLASS.get(task_class, [None])[0]
                if kind == "tool_intent"
                else None
            ),
        )
        for sid, kind, deps in steps_spec
    ]
    tool_intents = _TOOL_BY_CLASS.get(task_class, [])
    return Plan(
        schema_version=SCHEMA_VERSION,
        task_class=task_class,
        route=route.value,
        assumptions=["el pedido cabe en el alcance declarado; ante duda, escalar"],
        steps=steps,
        expected_evidence=[evidence_ref],
        risk="medium" if tool_intents else "low",
        tool_intents=list(tool_intents),
        worker_hints=[],
        budget=Budget(
            max_steps=min(len(steps), profile.max_steps),
            max_latency_ms=profile.max_plan_latency_ms,
            max_refine_iterations=profile.max_refine_iterations,
        ),
        stop_conditions=["budget_exhausted", "validator_rejected"],
    )
