"""Perfiles de hardware tipados (OMEGA).

LOW queda validado físicamente en esta misión (8 GB, sin GPU). MEDIUM/POWER
son políticas tipadas verificadas con fixtures/simulación — NO se afirma
validación física de hardware que no está disponible.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Profile:
    name: str
    # Límites del plan
    max_steps: int
    max_refine_iterations: int
    max_input_chars: int
    # Presupuestos
    max_plan_latency_ms: int
    # Capacidades locales
    allow_residual_llm: bool
    concurrency: int
    # Estado de validación (honesto: solo LOW se valida físicamente acá)
    physically_validated: bool


LOW = Profile(
    name="low",
    max_steps=10,
    max_refine_iterations=1,
    max_input_chars=4_000,
    max_plan_latency_ms=800,
    allow_residual_llm=False,  # opt-in vía env, nunca default en LOW
    concurrency=2,
    physically_validated=True,
)

MEDIUM = Profile(
    name="medium",
    max_steps=16,
    max_refine_iterations=2,
    max_input_chars=8_000,
    max_plan_latency_ms=1_500,
    allow_residual_llm=True,
    concurrency=4,
    physically_validated=False,  # fixtures only: no hay hardware de 16GB acá
)

POWER = Profile(
    name="power",
    max_steps=24,
    max_refine_iterations=2,
    max_input_chars=16_000,
    max_plan_latency_ms=2_500,
    allow_residual_llm=True,
    concurrency=8,
    physically_validated=False,  # fixtures only
)

PROFILES = {p.name: p for p in (LOW, MEDIUM, POWER)}
