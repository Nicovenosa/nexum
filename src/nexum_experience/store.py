"""ExperienceStore — SQLite WAL, versionado, con retención y dedup.

Contratos duros (SPEC-EXPERIENCE-POOL-001):
- WAL + busy_timeout + foreign_keys (PRAGMAs de la casa);
- schema_version en tabla meta + migrations idempotentes;
- integrity check al abrir; corrupción ⇒ renombrar `.corrupt-<ts>` y recrear
  (jamás perder el proceso por una DB rota);
- retención acotada: max_rows y max_age_days (crecimiento acotado por diseño);
- delete por tombstone (purge explícito aparte);
- dedup por feature_hash con contador (no se pierden repeticiones: se cuentan).
"""

from __future__ import annotations

import json
import sqlite3
import time
from dataclasses import asdict
from pathlib import Path

from .events import SCHEMA_VERSION, EventValidationError, ExperienceEvent

_BUSY_TIMEOUT_MS = 5_000
DEFAULT_MAX_ROWS = 200_000
DEFAULT_MAX_AGE_DAYS = 90
_RETENTION_EVERY = 500  # aplicar retención cada N inserts (barato y acotado)


class ExperienceStore:
    def __init__(
        self,
        db_path: Path | str,
        max_rows: int = DEFAULT_MAX_ROWS,
        max_age_days: int = DEFAULT_MAX_AGE_DAYS,
    ) -> None:
        self.db_path = Path(db_path)
        self.max_rows = max_rows
        self.max_age_days = max_age_days
        self._inserts_since_retention = 0
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = self._open_with_corruption_policy()

    # ── apertura / integridad ────────────────────────────────────────

    def _open_with_corruption_policy(self) -> sqlite3.Connection:
        try:
            conn = self._open_raw()
            row = conn.execute("PRAGMA integrity_check(1)").fetchone()
            if row and row[0] == "ok":
                return conn
            conn.close()
            raise sqlite3.DatabaseError("integrity_check falló")
        except sqlite3.DatabaseError:
            # Corrupción: preservar el archivo para forense y recrear.
            if self.db_path.exists():
                ts = int(time.time())
                self.db_path.rename(self.db_path.with_suffix(f".corrupt-{ts}"))
            return self._open_raw()

    def _open_raw(self) -> sqlite3.Connection:
        conn = sqlite3.connect(self.db_path, timeout=_BUSY_TIMEOUT_MS / 1000)
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA synchronous=NORMAL")
        conn.execute("PRAGMA foreign_keys=ON")
        conn.execute(f"PRAGMA busy_timeout={_BUSY_TIMEOUT_MS}")
        self._migrate(conn)
        return conn

    @staticmethod
    def _migrate(conn: sqlite3.Connection) -> None:
        """Migrations idempotentes: de cualquier versión anterior a la actual."""
        conn.execute(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)"
        )
        row = conn.execute(
            "SELECT value FROM meta WHERE key='schema_version'"
        ).fetchone()
        version = int(row[0]) if row else 0
        if version < 1:
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS events (
                    experience_id TEXT PRIMARY KEY,
                    ts REAL NOT NULL,
                    feature_hash TEXT NOT NULL,
                    dedup_count INTEGER NOT NULL DEFAULT 1,
                    deleted INTEGER NOT NULL DEFAULT 0,
                    payload TEXT NOT NULL
                )
                """
            )
            conn.execute("CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts)")
            conn.execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_events_fh ON events(feature_hash)"
            )
            conn.execute(
                "INSERT OR REPLACE INTO meta VALUES ('schema_version', ?)",
                (str(SCHEMA_VERSION),),
            )
        conn.commit()

    # ── escritura ────────────────────────────────────────────────────

    def record(self, event: ExperienceEvent) -> None:
        """Valida y persiste. Dedup: mismo feature_hash ⇒ contador += 1."""
        event.validate()
        fh = event.feature_hash()
        payload = json.dumps(asdict(event), ensure_ascii=False, separators=(",", ":"))
        with self._conn:
            cur = self._conn.execute(
                "UPDATE events SET dedup_count = dedup_count + 1, ts = ? "
                "WHERE feature_hash = ? AND deleted = 0",
                (event.ts, fh),
            )
            if cur.rowcount == 0:
                self._conn.execute(
                    "INSERT OR REPLACE INTO events "
                    "(experience_id, ts, feature_hash, dedup_count, deleted, payload) "
                    "VALUES (?, ?, ?, 1, 0, ?)",
                    (event.experience_id, event.ts, fh, payload),
                )
        self._inserts_since_retention += 1
        if self._inserts_since_retention >= _RETENTION_EVERY:
            self.apply_retention()

    def tombstone(self, experience_id: str) -> bool:
        with self._conn:
            cur = self._conn.execute(
                "UPDATE events SET deleted = 1 WHERE experience_id = ?",
                (experience_id,),
            )
        return cur.rowcount > 0

    def purge_tombstones(self) -> int:
        with self._conn:
            cur = self._conn.execute("DELETE FROM events WHERE deleted = 1")
        return cur.rowcount

    def apply_retention(self) -> int:
        """Borra lo más viejo que exceda max_rows y todo lo anterior a
        max_age_days. Devuelve filas eliminadas."""
        self._inserts_since_retention = 0
        removed = 0
        cutoff = time.time() - self.max_age_days * 86_400
        with self._conn:
            cur = self._conn.execute("DELETE FROM events WHERE ts < ?", (cutoff,))
            removed += cur.rowcount
            n = self._conn.execute(
                "SELECT COUNT(*) FROM events WHERE deleted = 0"
            ).fetchone()[0]
            if n > self.max_rows:
                cur = self._conn.execute(
                    "DELETE FROM events WHERE experience_id IN ("
                    "  SELECT experience_id FROM events WHERE deleted = 0 "
                    "  ORDER BY ts ASC LIMIT ?)",
                    (n - self.max_rows,),
                )
                removed += cur.rowcount
        return removed

    # ── lectura ──────────────────────────────────────────────────────

    def count(self, include_deleted: bool = False) -> int:
        q = "SELECT COUNT(*) FROM events" + (
            "" if include_deleted else " WHERE deleted = 0"
        )
        return self._conn.execute(q).fetchone()[0]

    def query(
        self,
        task_class: str | None = None,
        route: str | None = None,
        outcome: str | None = None,
        since_ts: float = 0.0,
        limit: int = 1_000,
    ) -> list[dict]:
        rows = self._conn.execute(
            "SELECT payload, dedup_count FROM events WHERE deleted = 0 AND ts >= ? "
            "ORDER BY ts DESC LIMIT ?",
            (since_ts, min(limit, 10_000)),
        ).fetchall()
        out = []
        for payload, dedup in rows:
            ev = json.loads(payload)
            ev["dedup_count"] = dedup
            if task_class and ev.get("task_class") != task_class:
                continue
            if route and ev.get("route_selected") != route:
                continue
            if outcome and ev.get("outcome") != outcome:
                continue
            out.append(ev)
        return out

    def export_replay_dataset(self, limit: int = 10_000) -> list[dict]:
        """Dataset sanitizado para replay de Nocturno: solo features/códigos.
        (El schema ya es sanitizado por construcción; esto además recorta a
        los campos de decisión.)"""
        keep = (
            "task_class",
            "risk_class",
            "source",
            "route_selected",
            "worker_selected",
            "provider_used",
            "outcome",
            "error_code",
            "input_chars",
            "input_hash",
            "policy_version",
            "latency_ms_total",
            "dedup_count",
        )
        return [{k: ev.get(k) for k in keep} for ev in self.query(limit=limit)]

    def db_size_bytes(self) -> int:
        try:
            return self.db_path.stat().st_size
        except OSError:
            return 0

    def close(self) -> None:
        try:
            self._conn.close()
        except sqlite3.Error:
            pass


__all__ = ["EventValidationError", "ExperienceStore"]
