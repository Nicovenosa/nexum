"""Planning pipeline del Hormiguero (OMEGA Fase 7, SPEC-HORMIGUERO-PLANNING-001).

Planner → Critic → Refiner (opcional, acotado) → Validator determinístico.

Reglas duras del contrato:
- el planner JAMÁS ejecuta tools, no decide seguridad, no autoaprueba;
- el Validator es determinístico y fail-closed: nada inválido pasa;
- perfil LOW: ≤10 pasos, ≤1 iteración de refine, validado físicamente;
- MEDIUM/POWER: políticas tipadas + fixtures (sin claim de validación física);
- Frozen Snapshot antes de refinar (rollback exacto);
- Error Enum tipado (nunca strings sueltos);
- ningún claim de éxito sin evidencia declarada en el plan.
"""

from .critic import CriticFinding, criticize
from .pipeline import PlanOutcome, run_pipeline
from .profiles import LOW, MEDIUM, POWER, Profile
from .refiner import refine
from .routes import POLICY_VERSION, RouteClass
from .schema import Plan, PlanStep, ValidationError
from .validator import validate

__all__ = [
    "LOW",
    "MEDIUM",
    "POWER",
    "POLICY_VERSION",
    "CriticFinding",
    "Plan",
    "PlanOutcome",
    "PlanStep",
    "Profile",
    "RouteClass",
    "ValidationError",
    "criticize",
    "refine",
    "run_pipeline",
    "validate",
]
