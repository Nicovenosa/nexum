"""Experience Pool + Evidence Layer (OMEGA Fase 10).

SPEC-EXPERIENCE-POOL-001 · SPEC-EVIDENCE-LAYER-001.

Separación dura de responsabilidades:
- MemoryGateway guarda RECUERDOS DEL USUARIO (autoridad: ADR-058).
- Experience Pool guarda EVIDENCIA OPERATIVA DEL SISTEMA (features, códigos,
  latencias, outcomes) para que Nocturno aprenda de forma controlada.
- Evidence Layer es la autoridad de evidencia reproducible (provenance
  obligatoria, integridad encadenada).

Este paquete NO importa nexum_memory_gateway ni comparte sus DB. Python
stdlib + sqlite3 únicamente.

PRIVACIDAD (vinculante): jamás prompts completos, respuestas completas,
audio, transcripciones, secretos ni tokens. Solo features, hashes, categorías
y reason codes. La validación de eventos lo fuerza mecánicamente.
"""

from .events import EventValidationError, ExperienceEvent
from .evidence import EvidenceRecord, EvidenceStore
from .store import ExperienceStore

__all__ = [
    "EventValidationError",
    "EvidenceRecord",
    "EvidenceStore",
    "ExperienceEvent",
    "ExperienceStore",
]
