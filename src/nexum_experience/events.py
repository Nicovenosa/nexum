"""ExperienceEvent v1 (SPEC-EXPERIENCE-POOL-001).

El schema NO tiene campos de contenido libre: todo es enum, código corto,
número o hash. Es imposible por construcción guardar un prompt/respuesta/
audio/secreto — y la validación además lo fuerza (campos código: charset
acotado y longitud máxima; canary tests en la suite).
"""

from __future__ import annotations

import hashlib
import re
import time
import uuid
from dataclasses import dataclass, field

SCHEMA_VERSION = 1

SOURCES = frozenset({"text", "voice", "tool"})
OUTCOMES = frozenset(
    {"success", "failure", "cancelled", "timeout", "rejected", "unknown"}
)
RISK_CLASSES = frozenset({"low", "medium", "high"})

# Campos "código": corto, sin espacios, charset seguro. Nunca contenido.
_CODE_RE = re.compile(r"^[a-z0-9_.:/-]{0,64}$")


class EventValidationError(ValueError):
    """El evento viola el contrato de privacidad/forma. No se persiste."""


def _require_code(name: str, value: str) -> str:
    if not isinstance(value, str) or not _CODE_RE.match(value):
        raise EventValidationError(
            f"{name}: debe ser un código corto [a-z0-9_.:/-]<=64, no contenido libre"
        )
    return value


def text_features(text: str) -> tuple[int, str]:
    """Features seguras de un texto: longitud + sha256 (jamás el texto)."""
    return len(text), hashlib.sha256(text.encode()).hexdigest()


@dataclass
class ExperienceEvent:
    # Identidad
    experience_id: str = field(default_factory=lambda: str(uuid.uuid4()))
    ts: float = field(default_factory=time.time)
    schema_version: int = SCHEMA_VERSION
    # Clasificación
    task_class: str = "generic"
    risk_class: str = "low"
    source: str = "text"
    # Contexto de versión (para que Nocturno compare políticas)
    hardware_profile: str = "low"
    runtime_version: str = ""
    policy_version: str = ""
    # Decisión
    route_selected: str = ""
    planner_used: bool = False
    worker_selected: str = ""
    provider_used: bool = False  # True = premium/provider; False = local
    tool_category: str = ""
    # Métricas
    latency_ms_total: int = 0
    latency_ms_routing: int = 0
    latency_ms_provider: int = 0
    token_estimate: int = 0
    cost_estimate_milli: int = 0  # milésimas de la unidad de costo
    # Resultado
    outcome: str = "unknown"
    validator_result: str = ""
    false_completion: bool = False
    error_code: str = ""
    retry_count: int = 0
    cancelled: bool = False
    user_approval: str = ""  # "", "approved", "rejected"
    user_feedback: str = ""  # "", "positive", "negative"
    rollback: bool = False
    # Features del input (JAMÁS el input)
    input_chars: int = 0
    input_hash: str = ""
    # Evidencia
    evidence_refs: list[str] = field(default_factory=list)

    def validate(self) -> None:
        if self.schema_version != SCHEMA_VERSION:
            raise EventValidationError("schema_version desconocida")
        if self.source not in SOURCES:
            raise EventValidationError(f"source inválido: {self.source!r}")
        if self.outcome not in OUTCOMES:
            raise EventValidationError(f"outcome inválido: {self.outcome!r}")
        if self.risk_class not in RISK_CLASSES:
            raise EventValidationError(f"risk_class inválido: {self.risk_class!r}")
        for name in (
            "task_class",
            "hardware_profile",
            "runtime_version",
            "policy_version",
            "route_selected",
            "worker_selected",
            "tool_category",
            "validator_result",
            "error_code",
            "user_approval",
            "user_feedback",
        ):
            _require_code(name, getattr(self, name))
        if self.input_hash and not re.match(r"^[0-9a-f]{16,64}$", self.input_hash):
            raise EventValidationError("input_hash: debe ser hex (hash), no contenido")
        for ref in self.evidence_refs:
            _require_code("evidence_ref", ref)
        for name in (
            "latency_ms_total",
            "latency_ms_routing",
            "latency_ms_provider",
            "token_estimate",
            "cost_estimate_milli",
            "retry_count",
            "input_chars",
        ):
            v = getattr(self, name)
            if not isinstance(v, int) or v < 0 or v > 10**9:
                raise EventValidationError(f"{name}: entero acotado requerido")

    def feature_hash(self) -> str:
        """Hash de dedup: misma decisión sobre el mismo input = mismo hash."""
        key = "|".join(
            (
                self.task_class,
                self.source,
                self.route_selected,
                self.worker_selected,
                self.outcome,
                self.error_code,
                self.input_hash,
                self.policy_version,
            )
        )
        return hashlib.sha256(key.encode()).hexdigest()
