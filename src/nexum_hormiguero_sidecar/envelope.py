"""TaskEnvelope mínimo (Sprint 0) + métrica naive vs envelope.

En Sprint 0 el envelope se construye pero el flujo Rust sigue mandando el
input original al provider (passthrough) — el envelope viaja en la
respuesta de /route para validar el contrato y medir el ratio. El uso real
del envelope como prompt es F1 (ver plan).
"""

from __future__ import annotations

import re
from typing import Any

# Redacción mínima de patrones de secretos (espejo reducido de
# ui/secret_redact.rs; la unificación en secret_patterns.toml es F1).
_SECRET_PATTERNS = [
    re.compile(r"\b(sk|tp|ghp|gho|xoxb)-[A-Za-z0-9_\-]{10,}\b"),
    re.compile(r"\bAIza[A-Za-z0-9_\-]{30,}\b"),
    re.compile(r"\bya29\.[A-Za-z0-9_\-\.]{20,}\b"),
    re.compile(r"\bBearer\s+[A-Za-z0-9_\-\.=]{16,}", re.I),
    re.compile(r"\b(access|refresh)_token[=:]\s*\S{12,}", re.I),
    re.compile(r"[?&](code|state)=[A-Za-z0-9_\-\.%]{10,}"),
]


def redact(text: str) -> tuple[str, int]:
    count = 0
    out = text
    for pat in _SECRET_PATTERNS:
        out, n = pat.subn("[REDACTED]", out)
        count += n
    return out, count


def build_task_envelope(
    text: str, source: str, escalation_reason: str, confidence: float
) -> dict[str, Any]:
    """Envelope Sprint 0: sin recall de memoria ni capabilities todavía
    (TODO F1: relevant_context_summary real + selected_project_context)."""
    normalized, redacted_count = redact(text.strip())
    naive_size = len(text.encode())
    envelope = {
        "user_intent": normalized[:200],
        "normalized_request": normalized,
        "relevant_context_summary": "",  # F1: recall acotado vía MemoryGateway
        "selected_project_context": [],  # F1
        "constraints": [],
        "safety_notes": (
            ["se redactaron posibles secretos del pedido"] if redacted_count else []
        ),
        "redaction_report": {
            "secrets_redacted": redacted_count,
            "paths_normalized": 0,
            "dropped_sections": ["historial completo (no incluido por diseño)"],
        },
        "required_output_format": "markdown",
        "allowed_tools": [],
        "disallowed_tools": [],
        "escalation_reason": escalation_reason,
        "confidence": confidence,
        "source": source,
    }
    envelope_size = len(_stable_json(envelope).encode())
    envelope["metrics"] = {
        "naive_size_bytes": naive_size,
        "envelope_size_bytes": envelope_size,
        # En Sprint 0 el envelope AGREGA overhead sobre un input pelado —
        # el ratio <0.3 recién tiene sentido en F1 cuando reemplace
        # historial+contexto naive. Se registra igual para tener baseline.
        "envelope_naive_ratio": round(envelope_size / naive_size, 2)
        if naive_size
        else None,
    }
    return envelope


def _stable_json(obj: Any) -> str:
    import json

    return json.dumps(obj, ensure_ascii=False, sort_keys=True)
