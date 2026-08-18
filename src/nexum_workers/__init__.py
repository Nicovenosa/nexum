"""Worker Registry tipado (OMEGA Fase 14, ciclo 6b).

Patrón: Advisor → Orchestrator → Worker.

Invariantes duros (testeados):
- ningún worker decide sus propios permisos ni se autoaprueba;
- ToolWorker declara intención: la aprobación vive en Security (runtime Rust);
- ProviderWorker conserva provider/modelo seleccionado (jamás lo cambia);
- MemoryWorker respeta confirmación explícita y scopes (contrato ADR-058);
- ValidationWorker no ejecuta nada: solo valida;
- VoiceAdapterWorker no crea un agent loop paralelo;
- worker desconocido ⇒ fail-closed;
- timeout/cancelación acotados; retry acotado; evidencia por outcome.
"""

from .registry import (
    WORKER_IDS,
    UnknownWorker,
    WorkerContract,
    WorkerOutcome,
    WorkerRegistry,
    WorkerRequest,
    build_default_registry,
)

__all__ = [
    "WORKER_IDS",
    "UnknownWorker",
    "WorkerContract",
    "WorkerOutcome",
    "WorkerRegistry",
    "WorkerRequest",
    "build_default_registry",
]
