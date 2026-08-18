"""MemoryGateway v0.1 — store SQLite productivo (SPEC-MEMORY-001, Backend A).

stdlib only. WAL, busy_timeout, transacciones, migraciones versionadas,
FTS5 con fallback LIKE declarado y medido (R-3), tombstone delete (D-8),
contradicciones sin overwrite con resolución explícita (D-12, incluye
keep_both), idempotencia (D-9), cuarentena ante corrupción (R-4: jamás
inventar datos), serialización obligatoria de toda operación (R-1,
invariante de diseño). Cero imports de nexum.* / packages/core (D-15).

Promovido selectivamente del spike M-1 (spike/memorygateway-v0.1 @ 91e1e28).
"""

from __future__ import annotations

import hashlib
import os
import sqlite3
import threading
import time
import uuid

SCHEMA_VERSION = 1
VALID_SCOPES = ("user", "project")
# Tombstone (no hard delete, D-8 de SPEC-MEMORY-001): preserva
# proveniencia/auditoría (ADR-058 "certeza auditable") y hace el delete
# repetido idempotente por diseño. Purge físico: post-v0.1 con NCP.
DELETE_MODE = "tombstone"


class GatewayError(Exception):
    def __init__(self, code: str, message: str, http: int = 400):
        super().__init__(message)
        self.code = code  # errores versionados: MG_<AREA>_<NN>
        self.message = message
        self.http = http


def _now_ms() -> int:
    return int(time.time() * 1000)


def _checksum(content: str) -> str:
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def _serialized(method):
    """R-1 (invariante de diseño, no parche): serializa TODA operación del
    store. Además versiona DB ocupada (MG_DB_02, retryable) y aplica la
    política R-4 ante corrupción: cuarentena + MG_DB_03, jamás datos
    inventados ni éxito parcial silencioso.
    """

    def inner(self, *args, **kwargs):
        with self._lock:
            if self.db_state == "quarantined":
                raise GatewayError(
                    "MG_DB_03",
                    "memoria no disponible: base en cuarentena "
                    f"({self.quarantined_path}); restaurar o crear base nueva "
                    "requiere accion explicita (/memoria reset)",
                    503,
                )
            try:
                return method(self, *args, **kwargs)
            except sqlite3.IntegrityError:
                raise  # constraint inesperada: fail-closed (MG_INT_99), no es corrupción
            except sqlite3.OperationalError as exc:
                if "locked" in str(exc) or "busy" in str(exc):
                    raise GatewayError(
                        "MG_DB_02", "base de datos ocupada, reintentar", 503
                    ) from exc
                self._quarantine()
                raise GatewayError(
                    "MG_DB_03",
                    "memoria no disponible: corrupción detectada, base aislada "
                    f"en {self.quarantined_path}",
                    503,
                ) from exc
            except sqlite3.DatabaseError as exc:
                self._quarantine()
                raise GatewayError(
                    "MG_DB_03",
                    "memoria no disponible: corrupción detectada, base aislada "
                    f"en {self.quarantined_path}",
                    503,
                ) from exc

    return inner


class MemoryStore:
    def __init__(self, db_path: str):
        self.db_path = db_path
        # R-1: una única conexión compartida entre los threads del server;
        # _lock serializa TODA operación (sin él se pierden writes bajo
        # concurrencia — HALLAZGO-1 del spike). Invariante de diseño.
        self._lock = threading.RLock()
        self.db_state = "ok"  # ok | quarantined
        self.quarantined_path: str | None = None
        self.fresh_after_quarantine = False
        try:
            self._open()
        except sqlite3.DatabaseError:
            # R-4: corrupción al abrir → cuarentena; NO se crea base nueva
            # automáticamente (requiere acción explícita del usuario).
            self._quarantine()

    def _open(self) -> None:
        self.conn = sqlite3.connect(self.db_path, check_same_thread=False)
        self.conn.execute("PRAGMA journal_mode=WAL")
        self.conn.execute("PRAGMA synchronous=NORMAL")
        self.conn.execute("PRAGMA foreign_keys=ON")
        self.conn.execute("PRAGMA busy_timeout=3000")
        # R-4: chequeo de integridad al abrir. quick_check es barato y
        # detecta header inválido/truncados/malformed.
        check = self.conn.execute("PRAGMA quick_check(1)").fetchone()
        if not check or check[0] != "ok":
            raise sqlite3.DatabaseError(f"quick_check: {check}")
        self.fts_enabled = self._detect_fts5()
        self._migrate()

    def _quarantine(self) -> None:
        """R-4: aislar la base corrupta SIN borrarla, detener escrituras,
        informar. La base nueva solo se crea vía reset_after_quarantine()
        (acción explícita del usuario)."""
        try:
            self.conn.close()
        except Exception:  # noqa: BLE001 — la conexión puede estar rota
            pass
        stamp = int(time.time())
        target = f"{self.db_path}.corrupt-{stamp}"
        for suffix in ("", "-wal", "-shm"):
            src = self.db_path + suffix
            if os.path.exists(src):
                try:
                    os.replace(src, target + suffix)
                except OSError:
                    pass
        self.db_state = "quarantined"
        self.quarantined_path = target

    def reset_after_quarantine(self) -> dict:
        """Acción EXPLÍCITA del usuario (R-4.4): crear base vacía tras la
        cuarentena. Jamás automática. La base aislada queda preservada."""
        with self._lock:
            if self.db_state != "quarantined":
                raise GatewayError("MG_DB_04", "no hay base en cuarentena", 409)
            self._open()
            self.db_state = "ok"
            self.fresh_after_quarantine = True
            return {
                "ok": True,
                "fresh_db": True,
                "quarantined_path": self.quarantined_path,
            }

    def _detect_fts5(self) -> bool:
        # R-3: NEXUM_MEMORY_FORCE_LIKE=1 fuerza el fallback LIKE (para
        # medición y para sistemas sin FTS5 — capacidad degradada DECLARADA).
        if os.environ.get("NEXUM_MEMORY_FORCE_LIKE") == "1":
            return False
        try:
            self.conn.execute(
                "CREATE VIRTUAL TABLE IF NOT EXISTS _fts_probe USING fts5(x)"
            )
            self.conn.execute("DROP TABLE _fts_probe")
            return True
        except sqlite3.OperationalError:
            return False

    @property
    def search_backend(self) -> str:
        return "FTS5" if self.fts_enabled else "LIKE_FALLBACK"

    def _migrate(self) -> None:
        with self.conn:
            self.conn.execute(
                "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)"
            )
            cur = self.conn.execute(
                "SELECT COALESCE(MAX(version),0) FROM schema_migrations"
            )
            current = cur.fetchone()[0]
            if current < 1:
                self.conn.executescript(
                    """
                    CREATE TABLE entries (
                        id TEXT PRIMARY KEY,
                        key TEXT,
                        content TEXT NOT NULL,
                        scope_type TEXT NOT NULL CHECK (scope_type IN ('user','project')),
                        scope_id TEXT NOT NULL,
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL,
                        source_type TEXT NOT NULL,
                        source_reference TEXT NOT NULL,
                        status TEXT NOT NULL DEFAULT 'active'
                            CHECK (status IN ('active','in_conflict','superseded','deleted')),
                        version INTEGER NOT NULL DEFAULT 1,
                        checksum TEXT NOT NULL,
                        contradiction_group TEXT,
                        idempotency_key TEXT
                    );
                    CREATE UNIQUE INDEX idx_idem ON entries(idempotency_key) WHERE idempotency_key IS NOT NULL;
                    CREATE INDEX idx_scope ON entries(scope_type, scope_id, status);
                    CREATE INDEX idx_key ON entries(key, scope_type, scope_id);
                    CREATE TABLE contradictions (
                        group_id TEXT PRIMARY KEY,
                        key TEXT NOT NULL,
                        scope_type TEXT NOT NULL,
                        scope_id TEXT NOT NULL,
                        status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','resolved')),
                        created_at INTEGER NOT NULL,
                        resolved_at INTEGER,
                        winner_id TEXT,
                        resolution_note TEXT
                    );
                    """
                )
                if self.fts_enabled:
                    self.conn.execute(
                        "CREATE VIRTUAL TABLE entries_fts USING fts5(content, id UNINDEXED)"
                    )
                self.conn.execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES (1, ?)",
                    (_now_ms(),),
                )

    # ── operaciones ──────────────────────────────────────────────────

    @_serialized
    def save(self, payload: dict) -> dict:
        if payload.get("confirmed") is not True:
            raise GatewayError(
                "MG_WRITE_01", "escritura estable requiere confirmed=true", 422
            )
        content = payload.get("content")
        if not isinstance(content, str) or not content.strip():
            raise GatewayError("MG_VALID_01", "content requerido (str no vacío)", 400)
        scope_type = payload.get("scope_type")
        if scope_type not in VALID_SCOPES:
            raise GatewayError("MG_SCOPE_01", "scope_type debe ser user|project", 400)
        scope_id = payload.get("scope_id")
        if not isinstance(scope_id, str) or not scope_id:
            raise GatewayError("MG_SCOPE_02", "scope_id requerido", 400)
        source_type = payload.get("source_type") or "user_explicit"
        source_reference = payload.get("source_reference") or "unknown"
        key = payload.get("key")
        idem = payload.get("idempotency_key")

        if idem:
            row = self.conn.execute(
                "SELECT id FROM entries WHERE idempotency_key=?", (idem,)
            ).fetchone()
            if row:
                return {
                    "ok": True,
                    "id": row[0],
                    "deduplicated": True,
                    "conflict": None,
                }

        now = _now_ms()
        eid = str(uuid.uuid4())
        conflict = None
        with self.conn:
            if key:
                prev = self.conn.execute(
                    "SELECT id, content, contradiction_group FROM entries "
                    "WHERE key=? AND scope_type=? AND scope_id=? AND status IN ('active','in_conflict')",
                    (key, scope_type, scope_id),
                ).fetchall()
                incompatible = [r for r in prev if r[1] != content]
                if incompatible:
                    group = incompatible[0][2] or str(uuid.uuid4())
                    self.conn.execute(
                        "INSERT OR IGNORE INTO contradictions(group_id,key,scope_type,scope_id,status,created_at) "
                        "VALUES (?,?,?,?, 'open', ?)",
                        (group, key, scope_type, scope_id, now),
                    )
                    for r in incompatible:
                        self.conn.execute(
                            "UPDATE entries SET status='in_conflict', contradiction_group=?, updated_at=? WHERE id=?",
                            (group, now, r[0]),
                        )
                    self.conn.execute(
                        "INSERT INTO entries(id,key,content,scope_type,scope_id,created_at,updated_at,"
                        "source_type,source_reference,status,version,checksum,contradiction_group,idempotency_key) "
                        "VALUES (?,?,?,?,?,?,?,?,?,'in_conflict',1,?,?,?)",
                        (
                            eid,
                            key,
                            content,
                            scope_type,
                            scope_id,
                            now,
                            now,
                            source_type,
                            source_reference,
                            _checksum(content),
                            group,
                            idem,
                        ),
                    )
                    conflict = self._conflict_view(group)
                    if self.fts_enabled:
                        self.conn.execute(
                            "INSERT INTO entries_fts(content,id) VALUES (?,?)",
                            (content, eid),
                        )
                    return {
                        "ok": True,
                        "id": eid,
                        "deduplicated": False,
                        "conflict": conflict,
                    }
            self.conn.execute(
                "INSERT INTO entries(id,key,content,scope_type,scope_id,created_at,updated_at,"
                "source_type,source_reference,status,version,checksum,contradiction_group,idempotency_key) "
                "VALUES (?,?,?,?,?,?,?,?,?,'active',1,?,NULL,?)",
                (
                    eid,
                    key,
                    content,
                    scope_type,
                    scope_id,
                    now,
                    now,
                    source_type,
                    source_reference,
                    _checksum(content),
                    idem,
                ),
            )
            if self.fts_enabled:
                self.conn.execute(
                    "INSERT INTO entries_fts(content,id) VALUES (?,?)", (content, eid)
                )
        return {"ok": True, "id": eid, "deduplicated": False, "conflict": None}

    def _row_to_entry(self, r) -> dict:
        return {
            "id": r[0],
            "key": r[1],
            "content": r[2],
            "scope_type": r[3],
            "scope_id": r[4],
            "created_at": r[5],
            "updated_at": r[6],
            "source_type": r[7],
            "source_reference": r[8],
            "status": r[9],
            "version": r[10],
            "checksum": r[11],
            "contradiction_group": r[12],
        }

    _COLS = (
        "id,key,content,scope_type,scope_id,created_at,updated_at,"
        "source_type,source_reference,status,version,checksum,contradiction_group"
    )

    def _require_scope(self, payload: dict) -> tuple:
        st, si = payload.get("scope_type"), payload.get("scope_id")
        if st not in VALID_SCOPES or not isinstance(si, str) or not si:
            raise GatewayError(
                "MG_SCOPE_01", "scope_type/scope_id requeridos y válidos", 400
            )
        return st, si

    @_serialized
    def recall(self, payload: dict) -> dict:
        st, si = self._require_scope(payload)
        q = payload.get("query")
        if not isinstance(q, str) or not q.strip():
            raise GatewayError("MG_VALID_02", "query requerida", 400)
        limit = min(int(payload.get("limit", 10)), 50)
        # `scores` queda vacío cuando no hay ranking disponible (camino LIKE).
        # Es la señal de "no sé qué tan relevante es esto", y el consumidor tiene
        # que poder distinguirla de "es poco relevante".
        scores: dict[str, float] = {}
        if self.fts_enabled:
            try:
                # bm25() ordena por RELEVANCIA. Antes se ordenaba por
                # `updated_at DESC`, o sea por lo más RECIENTE que matcheara —
                # con términos unidos por OR, eso devuelve cualquier entrada que
                # comparta una palabra, ordenada por fecha. Un recall que
                # devuelve algo irrelevante es peor que uno que no devuelve
                # nada: el consumidor no tenía forma de descartarlo porque no
                # había con qué medirlo.
                # bm25 de SQLite es negativo y más negativo = mejor; se invierte
                # para que score alto = más relevante.
                filas = self.conn.execute(
                    "SELECT id, -bm25(entries_fts) AS rel FROM entries_fts "
                    "WHERE entries_fts MATCH ? ORDER BY rel DESC LIMIT 200",
                    (" OR ".join(t for t in q.split() if t),),
                ).fetchall()
                ids = [r[0] for r in filas]
                scores = {r[0]: float(r[1]) for r in filas}
            except sqlite3.OperationalError:
                ids = []
            if ids:
                marks = ",".join("?" * len(ids))
                rows = self.conn.execute(
                    f"SELECT {self._COLS} FROM entries WHERE id IN ({marks}) "
                    "AND scope_type=? AND scope_id=? AND status IN ('active','in_conflict')",
                    (*ids, st, si),
                ).fetchall()
                # El orden lo pone bm25, no el ORDER BY de SQL: `IN (...)` no
                # preserva el orden de la lista.
                orden = {i: n for n, i in enumerate(ids)}
                rows = sorted(rows, key=lambda r: orden.get(r[0], 1 << 30))[:limit]
            else:
                rows = []
        else:
            rows = self.conn.execute(
                f"SELECT {self._COLS} FROM entries WHERE content LIKE ? "
                "AND scope_type=? AND scope_id=? AND status IN ('active','in_conflict') "
                "ORDER BY updated_at DESC LIMIT ?",
                (f"%{q}%", st, si, limit),
            ).fetchall()
        resultados = []
        for r in rows:
            e = self._row_to_entry(r)
            # `None` = sin ranking disponible, distinto de 0.0 = irrelevante.
            e["relevance"] = scores.get(e["id"])
            resultados.append(e)
        return {
            "ok": True,
            "results": resultados,
            "engine": self.search_backend,
        }

    @_serialized
    def get(self, payload: dict) -> dict:
        st, si = self._require_scope(payload)
        r = self.conn.execute(
            f"SELECT {self._COLS} FROM entries WHERE id=? AND scope_type=? AND scope_id=?",
            (payload.get("id"), st, si),
        ).fetchone()
        if not r:
            raise GatewayError("MG_GET_01", "entrada inexistente en este scope", 404)
        return {"ok": True, "entry": self._row_to_entry(r)}

    @_serialized
    def list(self, payload: dict) -> dict:
        st, si = self._require_scope(payload)
        limit = min(int(payload.get("limit", 50)), 200)
        rows = self.conn.execute(
            f"SELECT {self._COLS} FROM entries WHERE scope_type=? AND scope_id=? "
            "AND status != 'deleted' ORDER BY updated_at DESC LIMIT ?",
            (st, si, limit),
        ).fetchall()
        return {"ok": True, "results": [self._row_to_entry(r) for r in rows]}

    @_serialized
    def delete(self, payload: dict) -> dict:
        st, si = self._require_scope(payload)
        eid = payload.get("id")
        with self.conn:
            cur = self.conn.execute(
                "UPDATE entries SET status='deleted', updated_at=? "
                "WHERE id=? AND scope_type=? AND scope_id=? AND status != 'deleted'",
                (_now_ms(), eid, st, si),
            )
            exists = self.conn.execute(
                "SELECT 1 FROM entries WHERE id=? AND scope_type=? AND scope_id=?",
                (eid, st, si),
            ).fetchone()
        if not exists:
            raise GatewayError("MG_DEL_01", "entrada inexistente en este scope", 404)
        return {
            "ok": True,
            "deleted": bool(cur.rowcount),
            "mode": DELETE_MODE,
            "already_deleted": not bool(cur.rowcount),
        }

    def _conflict_view(self, group_id: str) -> dict:
        g = self.conn.execute(
            "SELECT group_id,key,scope_type,scope_id,status,created_at,resolved_at,winner_id,resolution_note "
            "FROM contradictions WHERE group_id=?",
            (group_id,),
        ).fetchone()
        entries = self.conn.execute(
            f"SELECT {self._COLS} FROM entries WHERE contradiction_group=? AND status != 'deleted'",
            (group_id,),
        ).fetchall()
        return {
            "group_id": g[0],
            "key": g[1],
            "scope_type": g[2],
            "scope_id": g[3],
            "status": g[4],
            "created_at": g[5],
            "resolved_at": g[6],
            "winner_id": g[7],
            "resolution_note": g[8],
            "entries": [self._row_to_entry(r) for r in entries],
        }

    @_serialized
    def propose_contradiction(self, payload: dict) -> dict:
        st, si = self._require_scope(payload)
        group = payload.get("group_id")
        if group:
            return {"ok": True, "conflict": self._conflict_view(group)}
        rows = self.conn.execute(
            "SELECT group_id FROM contradictions WHERE scope_type=? AND scope_id=? AND status='open'",
            (st, si),
        ).fetchall()
        return {"ok": True, "open_conflicts": [self._conflict_view(r[0]) for r in rows]}

    @_serialized
    def resolve_contradiction(self, payload: dict) -> dict:
        """Resolución EXPLÍCITA (D-12): winner (una gana, el resto superseded)
        o keep_both (ambas quedan activas con la resolución registrada).
        Nunca elección silenciosa de un modelo."""
        st, si = self._require_scope(payload)
        group = payload.get("group_id")
        mode = payload.get("resolution_mode") or "winner"
        note = payload.get("resolution_note") or "resolución explícita del usuario"
        g = self.conn.execute(
            "SELECT status FROM contradictions WHERE group_id=? AND scope_type=? AND scope_id=?",
            (group, st, si),
        ).fetchone()
        if not g:
            raise GatewayError("MG_CONF_01", "conflicto inexistente en este scope", 404)
        if g[0] == "resolved":
            raise GatewayError("MG_CONF_02", "conflicto ya resuelto", 409)
        now = _now_ms()
        if mode == "keep_both":
            with self.conn:
                self.conn.execute(
                    "UPDATE entries SET status='active', updated_at=? "
                    "WHERE contradiction_group=? AND status='in_conflict'",
                    (now, group),
                )
                self.conn.execute(
                    "UPDATE contradictions SET status='resolved', resolved_at=?, winner_id=NULL, "
                    "resolution_note=? WHERE group_id=?",
                    (now, f"[keep_both] {note}", group),
                )
            return {"ok": True, "conflict": self._conflict_view(group)}
        if mode != "winner":
            raise GatewayError(
                "MG_CONF_04", "resolution_mode debe ser winner|keep_both", 400
            )
        winner = payload.get("winner_id")
        w = self.conn.execute(
            "SELECT 1 FROM entries WHERE id=? AND contradiction_group=?",
            (winner, group),
        ).fetchone()
        if not w:
            raise GatewayError("MG_CONF_03", "winner_id no pertenece al conflicto", 400)
        with self.conn:
            self.conn.execute(
                "UPDATE entries SET status='active', updated_at=? WHERE id=?",
                (now, winner),
            )
            self.conn.execute(
                "UPDATE entries SET status='superseded', updated_at=? "
                "WHERE contradiction_group=? AND id != ? AND status='in_conflict'",
                (now, group, winner),
            )
            self.conn.execute(
                "UPDATE contradictions SET status='resolved', resolved_at=?, winner_id=?, resolution_note=? "
                "WHERE group_id=?",
                (now, winner, note, group),
            )
        return {"ok": True, "conflict": self._conflict_view(group)}

    @_serialized
    def stats(self) -> dict:
        n = self.conn.execute("SELECT COUNT(*) FROM entries").fetchone()[0]
        return {
            "entries": n,
            "search_backend": self.search_backend,
            "schema_version": SCHEMA_VERSION,
            "delete_mode": DELETE_MODE,
            "db_state": self.db_state,
            "fresh_after_quarantine": self.fresh_after_quarantine,
        }

    def close(self) -> None:
        try:
            self.conn.close()
        except Exception:  # noqa: BLE001 — puede estar en cuarentena
            pass
