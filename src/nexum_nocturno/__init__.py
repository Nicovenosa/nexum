"""Nexum Nocturno — aprendizaje controlado (OMEGA Fase 11, SPEC-NOCTURNO-001).

Ciclo obligatorio (nada salta etapas):
Experiencia → Experience Pool → Evidence Layer → hipótesis → candidato →
sandbox → benchmark → regresión → security → shadow → approval → promoción
o rechazo → rollback.

AUTOPROMOTE=OFF por defecto (vinculante en v0.1): la promoción SIEMPRE exige
el comando explícito approve(). Nocturno JAMÁS modifica immutable policies.
"""

from .candidates import (
    IMMUTABLE_POLICY_KEYS,
    Candidate,
    CandidateStore,
    InvalidTransition,
    PromotionState,
)
from .engine import Nocturno, NocturnoMode

__all__ = [
    "IMMUTABLE_POLICY_KEYS",
    "Candidate",
    "CandidateStore",
    "InvalidTransition",
    "Nocturno",
    "NocturnoMode",
    "PromotionState",
]
