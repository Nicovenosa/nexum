"""Evidence Layer — autoridad de evidencia reproducible (SPEC-EVIDENCE-LAYER-001).

Cada registro exige provenance y queda encadenado por hash (prev_hash +
payload → record_hash): cualquier alteración rompe la cadena y `verify_chain`
lo detecta. Nocturno solo puede aprender de eventos/evidencia con provenance
y schema válidos.
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
import time
import uuid
from dataclasses import asdict, dataclass, field
from pathlib import Path

SCHEMA_VERSION = 1
_BUSY_TIMEOUT_MS = 5_000

SOURCES = frozenset(
    {
        "test",
        "benchmark",
        "failure_injection",
        "user_feedback",
        "validator",
        "observed_result",
    }
)
RESULTS = frozenset({"pass", "fail", "improved", "regressed", "neutral"})


class EvidenceValidationError(ValueError):
    pass


@dataclass
class EvidenceRecord:
    evidence_id: str = field(default_factory=lambda: str(uuid.uuid4()))
    ts: float = field(default_factory=time.time)
    schema_version: int = SCHEMA_VERSION
    source: str = "observed_result"  # SOURCES
    environment: str = ""  # ej "low/linux-x86_64"
    artifact: str = ""  # ej "nexum-0.1.1-rc.1"
    runtime_version: str = ""
    policy_version: str = ""
    result: str = "neutral"  # RESULTS
    confidence: float = 0.0  # [0,1]
    provenance: str = ""  # OBLIGATORIA: comando/test/harness que la produjo
    metric_name: str = ""
    metric_value: float = 0.0
    baseline_value: float = 0.0
    regression_ref: str = ""

    def validate(self) -> None:
        if self.schema_version != SCHEMA_VERSION:
            raise EvidenceValidationError("schema_version desconocida")
        if self.source not in SOURCES:
            raise EvidenceValidationError(f"source inválido: {self.source!r}")
        if self.result not in RESULTS:
            raise EvidenceValidationError(f"result inválido: {self.result!r}")
        if not (0.0 <= self.confidence <= 1.0):
            raise EvidenceValidationError("confidence fuera de [0,1]")
        if not self.provenance.strip():
            raise EvidenceValidationError("provenance es OBLIGATORIA")
        for name in ("environment", "artifact", "provenance", "metric_name"):
            v = getattr(self, name)
            if len(v) > 256 or "\n" in v:
                raise EvidenceValidationError(f"{name}: corto y sin saltos de línea")


class EvidenceStore:
    def __init__(self, db_path: Path | str) -> None:
        self.db_path = Path(db_path)
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(self.db_path, timeout=_BUSY_TIMEOUT_MS / 1000)
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._conn.execute("PRAGMA foreign_keys=ON")
        self._conn.execute(f"PRAGMA busy_timeout={_BUSY_TIMEOUT_MS}")
        self._migrate()

    def _migrate(self) -> None:
        self._conn.execute(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)"
        )
        row = self._conn.execute(
            "SELECT value FROM meta WHERE key='schema_version'"
        ).fetchone()
        if not row:
            self._conn.execute(
                """
                CREATE TABLE IF NOT EXISTS evidence (
                    seq INTEGER PRIMARY KEY AUTOINCREMENT,
                    evidence_id TEXT UNIQUE NOT NULL,
                    ts REAL NOT NULL,
                    payload TEXT NOT NULL,
                    prev_hash TEXT NOT NULL,
                    record_hash TEXT NOT NULL
                )
                """
            )
            self._conn.execute(
                "INSERT OR REPLACE INTO meta VALUES ('schema_version', ?)",
                (str(SCHEMA_VERSION),),
            )
        self._conn.commit()

    def append(self, record: EvidenceRecord) -> str:
        """Agrega evidencia encadenada. Devuelve el record_hash."""
        record.validate()
        payload = json.dumps(asdict(record), ensure_ascii=False, sort_keys=True)
        with self._conn:
            prev = self._conn.execute(
                "SELECT record_hash FROM evidence ORDER BY seq DESC LIMIT 1"
            ).fetchone()
            prev_hash = prev[0] if prev else "genesis"
            record_hash = hashlib.sha256((prev_hash + payload).encode()).hexdigest()
            self._conn.execute(
                "INSERT INTO evidence (evidence_id, ts, payload, prev_hash, record_hash) "
                "VALUES (?, ?, ?, ?, ?)",
                (record.evidence_id, record.ts, payload, prev_hash, record_hash),
            )
        return record_hash

    def verify_chain(self) -> tuple[bool, int]:
        """Recorre toda la cadena. Devuelve (ok, filas_verificadas)."""
        rows = self._conn.execute(
            "SELECT payload, prev_hash, record_hash FROM evidence ORDER BY seq ASC"
        ).fetchall()
        prev = "genesis"
        for i, (payload, prev_hash, record_hash) in enumerate(rows):
            if prev_hash != prev:
                return False, i
            expected = hashlib.sha256((prev_hash + payload).encode()).hexdigest()
            if expected != record_hash:
                return False, i
            prev = record_hash
        return True, len(rows)

    def query(self, source: str | None = None, limit: int = 1_000) -> list[dict]:
        rows = self._conn.execute(
            "SELECT payload FROM evidence ORDER BY seq DESC LIMIT ?",
            (min(limit, 10_000),),
        ).fetchall()
        out = [json.loads(p) for (p,) in rows]
        if source:
            out = [r for r in out if r.get("source") == source]
        return out

    def count(self) -> int:
        return self._conn.execute("SELECT COUNT(*) FROM evidence").fetchone()[0]

    def close(self) -> None:
        try:
            self._conn.close()
        except sqlite3.Error:
            pass
