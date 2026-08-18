"""Registry tipado de workers (SPEC en WORKER_REGISTRY_REPORT).

Cada worker es un CONTRATO verificable (id, schemas, capabilities, timeout,
retry, budget, evidence, health, error mapping, version). La ejecución real
de tools/provider vive en el runtime Rust — esta capa coordina y tipa; el
handler de cada worker acá es la referencia local (y el punto de test).
"""

from __future__ import annotations

import threading
import time
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Callable


class WorkerError(str, Enum):
    TIMEOUT = "timeout"
    CANCELLED = "cancelled"
    INVALID_INPUT = "invalid_input"
    INVALID_OUTPUT = "invalid_output"
    UNAVAILABLE = "unavailable"
    PERMISSION_REQUIRED = "permission_required"
    INTERNAL = "internal"


class UnknownWorker(KeyError):
    """Worker no registrado ⇒ fail-closed (jamás inventar capacidades)."""


@dataclass
class WorkerRequest:
    worker_id: str
    payload: dict[str, Any]
    deadline_ms: int = 1_000
    cancel_event: threading.Event | None = None


@dataclass
class WorkerOutcome:
    ok: bool
    worker_id: str
    error: WorkerError | None = None
    output: dict[str, Any] = field(default_factory=dict)
    latency_ms: float = 0.0
    retries: int = 0
    evidence: dict[str, Any] = field(default_factory=dict)


# Handler de referencia: recibe payload validado y devuelve output dict.
Handler = Callable[[dict[str, Any]], dict[str, Any]]


@dataclass(frozen=True)
class WorkerContract:
    worker_id: str
    version: str
    capabilities: tuple[str, ...]
    input_keys: frozenset[str]  # claves obligatorias del payload
    output_keys: frozenset[str]  # claves garantizadas del output
    timeout_ms: int
    max_retries: int
    resource_budget_note: str
    requires_approval: bool  # True ⇒ la salida SIEMPRE es una intención
    handler: Handler


WORKER_IDS = (
    "local_fast",
    "local_planner",
    "provider",
    "tool",
    "memory",
    "voice_adapter",
    "validation",
)


class WorkerRegistry:
    def __init__(self) -> None:
        self._contracts: dict[str, WorkerContract] = {}

    def register(self, contract: WorkerContract) -> None:
        self._contracts[contract.worker_id] = contract

    def contract(self, worker_id: str) -> WorkerContract:
        try:
            return self._contracts[worker_id]
        except KeyError as e:
            raise UnknownWorker(worker_id) from e

    def health(self) -> dict[str, str]:
        return {wid: "registered" for wid in sorted(self._contracts)}

    def dispatch(self, req: WorkerRequest) -> WorkerOutcome:
        """Ejecuta el contrato con timeout real (thread), cancelación
        cooperativa y retry acotado. Fail-closed en todo lo dudoso."""
        try:
            contract = self.contract(req.worker_id)
        except UnknownWorker:
            return WorkerOutcome(
                ok=False,
                worker_id=req.worker_id,
                error=WorkerError.INVALID_INPUT,
                evidence={"reason": "unknown_worker_fail_closed"},
            )
        missing = contract.input_keys - set(req.payload)
        if missing:
            return WorkerOutcome(
                ok=False,
                worker_id=req.worker_id,
                error=WorkerError.INVALID_INPUT,
                evidence={"missing_keys": sorted(missing)},
            )
        deadline = min(req.deadline_ms, contract.timeout_ms)
        t0 = time.perf_counter()
        retries = 0
        while True:
            if req.cancel_event is not None and req.cancel_event.is_set():
                return WorkerOutcome(
                    ok=False,
                    worker_id=req.worker_id,
                    error=WorkerError.CANCELLED,
                    latency_ms=(time.perf_counter() - t0) * 1000,
                    retries=retries,
                )
            result: dict[str, Any] = {}
            error: list[BaseException] = []

            def run() -> None:
                try:
                    result.update(contract.handler(req.payload))
                except BaseException as e:  # noqa: BLE001 (error mapping abajo)
                    error.append(e)

            th = threading.Thread(target=run, daemon=True)
            th.start()
            remaining = deadline / 1000 - (time.perf_counter() - t0)
            th.join(timeout=max(remaining, 0.001))
            if th.is_alive():
                # Timeout duro: el thread queda daemon (no bloquea el exit) y
                # el outcome es timeout. Un worker cancelado no "continúa"
                # lógicamente: su resultado se descarta SIEMPRE.
                return WorkerOutcome(
                    ok=False,
                    worker_id=req.worker_id,
                    error=WorkerError.TIMEOUT,
                    latency_ms=(time.perf_counter() - t0) * 1000,
                    retries=retries,
                    evidence={"deadline_ms": deadline},
                )
            if error:
                if retries < contract.max_retries:
                    retries += 1
                    continue
                return WorkerOutcome(
                    ok=False,
                    worker_id=req.worker_id,
                    error=WorkerError.INTERNAL,
                    latency_ms=(time.perf_counter() - t0) * 1000,
                    retries=retries,
                    evidence={"error_type": type(error[0]).__name__},
                )
            missing_out = contract.output_keys - set(result)
            if missing_out:
                return WorkerOutcome(
                    ok=False,
                    worker_id=req.worker_id,
                    error=WorkerError.INVALID_OUTPUT,
                    latency_ms=(time.perf_counter() - t0) * 1000,
                    retries=retries,
                    evidence={"missing_keys": sorted(missing_out)},
                )
            return WorkerOutcome(
                ok=True,
                worker_id=req.worker_id,
                output=result,
                latency_ms=(time.perf_counter() - t0) * 1000,
                retries=retries,
                evidence={"worker_version": contract.version},
            )


# ── Handlers de referencia (sin permisos propios, sin ejecución real) ────


def _local_fast(payload: dict[str, Any]) -> dict[str, Any]:
    from nexum_hormiguero_sidecar.classifier import classify, local_answer

    c = classify(str(payload["text"]))
    ans = local_answer(c.intent, str(payload.get("locale", "es")))
    if c.can_handle_local and not c.should_escalate and ans:
        return {"decision": "local_answer", "answer": ans}
    return {"decision": "escalate", "answer": ""}


def _local_planner(payload: dict[str, Any]) -> dict[str, Any]:
    from nexum_hormiguero_sidecar.planning import LOW, run_pipeline

    outcome = run_pipeline(
        str(payload["text"]), str(payload.get("task_class", "generic")), LOW
    )
    return {"status": outcome.status, "plan": outcome.public_dict()["plan"]}


def _provider(payload: dict[str, Any]) -> dict[str, Any]:
    # INVARIANTE: conserva provider/modelo seleccionados. Esta capa NO llama
    # a la red: construye la intención tipada que el runtime Rust ejecuta.
    return {
        "intent": "provider_call",
        "provider": str(payload["provider"]),
        "model": str(payload["model"]),
        "objective_hash": __import__("hashlib")
        .sha256(str(payload["text"]).encode())
        .hexdigest()[:16],
    }


def _tool(payload: dict[str, Any]) -> dict[str, Any]:
    # INVARIANTE: el ToolWorker DECLARA la intención; Security aprueba.
    return {
        "intent": "tool_call_proposal",
        "tool": str(payload["tool"]),
        "requires_approval": True,
    }


def _memory(payload: dict[str, Any]) -> dict[str, Any]:
    # INVARIANTE: stable write exige confirmación explícita (ADR-058).
    op = str(payload["op"])
    if op == "save" and not payload.get("user_confirmed", False):
        return {"intent": "memory_save_proposal", "requires_confirmation": True}
    return {"intent": f"memory_{op}", "requires_confirmation": False}


def _voice_adapter(payload: dict[str, Any]) -> dict[str, Any]:
    # INVARIANTE: la voz es CLIENTE del runtime: no crea agent loop paralelo.
    return {"intent": "speak", "text_chars": len(str(payload["text"]))}


def _validation(payload: dict[str, Any]) -> dict[str, Any]:
    from nexum_hormiguero_sidecar.planning import LOW, validate

    _, result = validate(payload["plan"], LOW)
    return {"valid": result.ok, "errors": [e.value for e in result.errors]}


def build_default_registry() -> WorkerRegistry:
    reg = WorkerRegistry()
    reg.register(
        WorkerContract(
            worker_id="local_fast",
            version="1.0",
            capabilities=("classify", "local_answer"),
            input_keys=frozenset({"text"}),
            output_keys=frozenset({"decision", "answer"}),
            timeout_ms=100,
            max_retries=0,
            resource_budget_note="cpu-only sub-ms",
            requires_approval=False,
            handler=_local_fast,
        )
    )
    reg.register(
        WorkerContract(
            worker_id="local_planner",
            version="1.0",
            capabilities=("plan",),
            input_keys=frozenset({"text"}),
            output_keys=frozenset({"status", "plan"}),
            timeout_ms=800,
            max_retries=0,
            resource_budget_note="LOW profile 800ms",
            requires_approval=False,
            handler=_local_planner,
        )
    )
    reg.register(
        WorkerContract(
            worker_id="provider",
            version="1.0",
            capabilities=("reasoning",),
            input_keys=frozenset({"text", "provider", "model"}),
            output_keys=frozenset({"intent", "provider", "model"}),
            timeout_ms=1_000,
            max_retries=1,
            resource_budget_note="intención, sin red acá",
            requires_approval=False,
            handler=_provider,
        )
    )
    reg.register(
        WorkerContract(
            worker_id="tool",
            version="1.0",
            capabilities=("tool_intent",),
            input_keys=frozenset({"tool"}),
            output_keys=frozenset({"intent", "requires_approval"}),
            timeout_ms=200,
            max_retries=0,
            resource_budget_note="intención, Security aprueba",
            requires_approval=True,
            handler=_tool,
        )
    )
    reg.register(
        WorkerContract(
            worker_id="memory",
            version="1.0",
            capabilities=("save_proposal", "recall"),
            input_keys=frozenset({"op"}),
            output_keys=frozenset({"intent"}),
            timeout_ms=500,
            max_retries=0,
            resource_budget_note="contrato ADR-058",
            requires_approval=True,
            handler=_memory,
        )
    )
    reg.register(
        WorkerContract(
            worker_id="voice_adapter",
            version="1.0",
            capabilities=("tts_intent",),
            input_keys=frozenset({"text"}),
            output_keys=frozenset({"intent"}),
            timeout_ms=300,
            max_retries=0,
            resource_budget_note="cliente del runtime",
            requires_approval=False,
            handler=_voice_adapter,
        )
    )
    reg.register(
        WorkerContract(
            worker_id="validation",
            version="1.0",
            capabilities=("validate_plan",),
            input_keys=frozenset({"plan"}),
            output_keys=frozenset({"valid", "errors"}),
            timeout_ms=200,
            max_retries=0,
            resource_budget_note="determinístico puro",
            requires_approval=False,
            handler=_validation,
        )
    )
    return reg
