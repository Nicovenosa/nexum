"""Candidatos y máquina de estados de promoción (SPEC-NOCTURNO-001)."""

from __future__ import annotations

import json
import sqlite3
import time
import uuid
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any


class PromotionState(str, Enum):
    PROPOSED = "proposed"
    SANDBOXED = "sandboxed"
    BENCHMARKED = "benchmarked"
    REGRESSION_VALIDATED = "regression_validated"
    SECURITY_VALIDATED = "security_validated"
    SHADOW = "shadow"
    APPROVAL_PENDING = "approval_pending"
    PROMOTED = "promoted"
    REJECTED = "rejected"
    ROLLED_BACK = "rolled_back"
    SUPERSEDED = "superseded"


# Transiciones válidas: NADA pasa de PROPOSED a PROMOTED directo.
VALID_TRANSITIONS: dict[PromotionState, frozenset[PromotionState]] = {
    PromotionState.PROPOSED: frozenset(
        {PromotionState.SANDBOXED, PromotionState.REJECTED}
    ),
    PromotionState.SANDBOXED: frozenset(
        {PromotionState.BENCHMARKED, PromotionState.REJECTED}
    ),
    PromotionState.BENCHMARKED: frozenset(
        {PromotionState.REGRESSION_VALIDATED, PromotionState.REJECTED}
    ),
    PromotionState.REGRESSION_VALIDATED: frozenset(
        {PromotionState.SECURITY_VALIDATED, PromotionState.REJECTED}
    ),
    PromotionState.SECURITY_VALIDATED: frozenset(
        {PromotionState.SHADOW, PromotionState.REJECTED}
    ),
    PromotionState.SHADOW: frozenset(
        {PromotionState.APPROVAL_PENDING, PromotionState.REJECTED}
    ),
    PromotionState.APPROVAL_PENDING: frozenset(
        {PromotionState.PROMOTED, PromotionState.REJECTED}
    ),
    PromotionState.PROMOTED: frozenset(
        {PromotionState.ROLLED_BACK, PromotionState.SUPERSEDED}
    ),
    PromotionState.REJECTED: frozenset(),
    PromotionState.ROLLED_BACK: frozenset(),
    PromotionState.SUPERSEDED: frozenset(),
}

CANDIDATE_KINDS = frozenset(
    {"routing_rule", "threshold", "residual_toggle", "cache_ttl", "budget"}
)

# Políticas INMUTABLES: Nocturno jamás puede tocar estas claves. Un candidato
# cuyo payload las mencione se rechaza en el security gate, sin excepción.
IMMUTABLE_POLICY_KEYS = frozenset(
    {
        "hitl",
        "yolo",
        "permissions",
        "secret_handling",
        "sandbox",
        "risk_classes",
        "provider_credentials",
        "spending_caps",
        "token_permissions",
        "loopback_only",
        "stable_memory_writes",
        "scopes",
        "security_rules",
        "tool_approval",
        "evidence_integrity",
    }
)


class InvalidTransition(RuntimeError):
    pass


@dataclass
class Candidate:
    candidate_id: str = field(default_factory=lambda: str(uuid.uuid4()))
    ts: float = field(default_factory=time.time)
    kind: str = "threshold"
    hypothesis_code: str = ""  # código corto, jamás contenido
    payload: dict[str, Any] = field(default_factory=dict)
    policy_version_base: str = ""
    state: PromotionState = PromotionState.PROPOSED
    baseline_metrics: dict[str, float] = field(default_factory=dict)
    candidate_metrics: dict[str, float] = field(default_factory=dict)
    shadow_metrics: dict[str, float] = field(default_factory=dict)
    frozen_snapshot: dict[str, Any] = field(default_factory=dict)
    rollback_pointer: str = ""  # policy_version a restaurar
    evidence_ids: list[str] = field(default_factory=list)
    reject_reason: str = ""

    def transition(self, new_state: PromotionState) -> None:
        allowed = VALID_TRANSITIONS[self.state]
        if new_state not in allowed:
            raise InvalidTransition(
                f"{self.state.value} → {new_state.value} no permitido "
                f"(válidos: {sorted(s.value for s in allowed)})"
            )
        self.state = new_state


class CandidateStore:
    """Persistencia SQLite (WAL) de candidatos."""

    def __init__(self, db_path: Path | str) -> None:
        self.db_path = Path(db_path)
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(self.db_path, timeout=5)
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA busy_timeout=5000")
        self._conn.execute(
            "CREATE TABLE IF NOT EXISTS candidates ("
            "candidate_id TEXT PRIMARY KEY, state TEXT NOT NULL, payload TEXT NOT NULL)"
        )
        self._conn.commit()

    def save(self, cand: Candidate) -> None:
        data = dict(cand.__dict__)
        data["state"] = cand.state.value
        with self._conn:
            self._conn.execute(
                "INSERT OR REPLACE INTO candidates VALUES (?, ?, ?)",
                (cand.candidate_id, cand.state.value, json.dumps(data)),
            )

    def load(self, candidate_id: str) -> Candidate | None:
        row = self._conn.execute(
            "SELECT payload FROM candidates WHERE candidate_id = ?", (candidate_id,)
        ).fetchone()
        if not row:
            return None
        data = json.loads(row[0])
        data["state"] = PromotionState(data["state"])
        return Candidate(**data)

    def list_by_state(self, state: PromotionState | None = None) -> list[Candidate]:
        if state is None:
            rows = self._conn.execute("SELECT payload FROM candidates").fetchall()
        else:
            rows = self._conn.execute(
                "SELECT payload FROM candidates WHERE state = ?", (state.value,)
            ).fetchall()
        out = []
        for (payload,) in rows:
            data = json.loads(payload)
            data["state"] = PromotionState(data["state"])
            out.append(Candidate(**data))
        return out

    def close(self) -> None:
        try:
            self._conn.close()
        except sqlite3.Error:
            pass
