"""Rutas tipadas y reason codes versionados (OMEGA Fase 6/7).

POLICY_VERSION identifica la política de routing/planning vigente; viaja en
Evidence/Experience para que Nocturno pueda comparar versiones.
"""

from __future__ import annotations

from enum import Enum

POLICY_VERSION = "hormiguero-planning-001/v1"


class RouteClass(str, Enum):
    """Rutas tipadas del Hormiguero (compatibilidad conceptual FAST/MEDIUM/COMPLEX)."""

    LOCAL_FAST = "local_fast"  # trivial: respuesta local inmediata (FAST)
    LOCAL_PLAN = "local_plan"  # plan corto ejecutable localmente (MEDIUM)
    TOOL_INTENT = "tool_intent"  # requiere tools del runtime (owner: Tools)
    MEMORY_INTENT = "memory_intent"  # requiere MemoryGateway (owner: Memory)
    PREMIUM_REASONING = "premium_reasoning"  # modelo principal (COMPLEX)
    ASK_USER = "ask_user"  # falta contexto: preguntar, no adivinar
    PASSTHROUGH = "passthrough"  # sin decisión local confiable: flujo normal
    DENY_BY_POLICY = "deny_by_policy"  # bloqueado por política (nunca por LLM)


# Rutas que un plan validado puede declarar como destino. DENY/ASK no llevan
# plan; LOCAL_FAST no necesita plan (respuesta enlatada).
PLAN_ALLOWED_ROUTES = frozenset(
    {
        RouteClass.LOCAL_PLAN,
        RouteClass.TOOL_INTENT,
        RouteClass.MEMORY_INTENT,
        RouteClass.PREMIUM_REASONING,
    }
)


class ReasonCode(str, Enum):
    """Reason codes públicos (seguros: sin reasoning interno, sin contenido)."""

    TRIVIAL_LOCAL = "trivial_local"
    COMPLEX_ESCALATED = "complex_escalated"
    FAIL_SAFE_ESCALATED = "fail_safe_escalated"
    PLAN_VALIDATED = "plan_validated"
    PLAN_REFINED = "plan_refined"
    PLAN_REJECTED = "plan_rejected"
    POLICY_DENIED = "policy_denied"
    NEEDS_USER_INPUT = "needs_user_input"
