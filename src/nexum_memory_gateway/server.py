"""Servidor HTTP del MemoryGateway — stdlib, loopback only, token obligatorio.

Seguridad (SEC-MEMORY-001..003):
- bind exclusivo 127.0.0.1, puerto efímero publicado en runtime_dir;
- toda request salvo GET /health exige X-Nexum-Memory-Token (0600);
- jamás se loggea token, contenido de memorias ni bodies;
- body ≤ 256 KiB; payload inválido fail-closed con error versionado;
- sin red externa: cero egress (offline, D del contrato).
"""

from __future__ import annotations

import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from . import __version__
from .auth import SERVICE_NAME, TOKEN_HEADER
from .watchdog import Activity
from .store import GatewayError, MemoryStore

_START = time.monotonic()
_MAX_BODY = 256 * 1024  # 256 KiB (límite del contrato)


class Counters:
    """Observabilidad mínima (FASE 12 M-3): números agregados por instancia.

    Jamás contenido de recuerdos, tokens ni secretos. p50/p95 sobre ventana.
    """

    _WINDOW = 512

    FIELDS = (
        "sidecar_starts",
        "singleton_reuse",
        "stale_cleanup",
        "saves_confirmed",
        "saves_rejected",
        "recalls",
        "lists",
        "gets",
        "deletes",
        "contradictions_detected",
        "contradictions_resolved",
        "auth_failures",
        "validation_failures",
        "db_busy",
        "db_corruption",
    )

    def __init__(self) -> None:
        self._lock = threading.Lock()
        for f in self.FIELDS:
            setattr(self, f, 0)
        self._latencies_ms: list[float] = []

    def bump(self, field: str) -> None:
        with self._lock:
            setattr(self, field, getattr(self, field) + 1)

    def record_latency(self, ms: float) -> None:
        with self._lock:
            self._latencies_ms.append(ms)
            if len(self._latencies_ms) > self._WINDOW:
                del self._latencies_ms[: -self._WINDOW]

    def snapshot(self) -> dict[str, Any]:
        with self._lock:
            lat = sorted(self._latencies_ms)
            n = len(lat)

            def pct(p: int) -> float | None:
                if not lat:
                    return None
                return round(lat[min(n * p // 100, n - 1)], 2)

            out: dict[str, Any] = {f: getattr(self, f) for f in self.FIELDS}
            out.update(
                {
                    "latency_window": n,
                    "latency_p50_ms": pct(50),
                    "latency_p95_ms": pct(95),
                }
            )
            return out


class GatewayHandler(BaseHTTPRequestHandler):
    server_version = "nexum-memory-gateway"
    # Inyectados por build_server():
    token: str = ""
    store: MemoryStore
    counters: Counters
    activity: Activity

    def _json(self, code: int, payload: dict[str, Any]) -> None:
        # Toda respuesta cuenta como actividad (watchdog idle-TTL, B-1).
        self.activity.touch()
        body = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _authed(self) -> bool:
        import hmac

        provided = self.headers.get(TOKEN_HEADER, "")
        return bool(self.token) and hmac.compare_digest(provided, self.token)

    def _read_body(self) -> dict[str, Any] | None:
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            return None
        if length <= 0:
            return None
        if length > _MAX_BODY:
            return {"__too_large__": True}
        try:
            data = json.loads(self.rfile.read(length).decode())
        except (json.JSONDecodeError, UnicodeDecodeError):
            return None
        return data if isinstance(data, dict) else None

    def log_message(self, fmt: str, *args: Any) -> None:
        # Log seguro: nada. Jamás bodies, headers, token ni contenido.
        pass

    def do_GET(self) -> None:  # noqa: N802 (firma de BaseHTTPRequestHandler)
        if self.path == "/health":
            self._json(
                200,
                {
                    "ok": True,
                    "service": SERVICE_NAME,
                    "version": __version__,
                    "contract": "v0.1",
                    "uptime_ms": int((time.monotonic() - _START) * 1000),
                    "search_backend": self.store.search_backend,
                    "db_state": self.store.db_state,
                    "quarantined_path": self.store.quarantined_path,
                },
            )
            return
        if not self._authed():
            self.counters.bump("auth_failures")
            self._json(
                401, {"ok": False, "code": "MG_AUTH_01", "message": "unauthorized"}
            )
            return
        if self.path == "/status":
            try:
                stats = self.store.stats()
            except GatewayError as e:
                stats = {"db_state": "quarantined", "error": e.code}
            self._json(
                200,
                {"ok": True, "stats": stats, "counters": self.counters.snapshot()},
            )
            return
        self._json(404, {"ok": False, "code": "MG_HTTP_04", "message": "not_found"})

    def do_POST(self) -> None:  # noqa: N802
        if not self._authed():
            self.counters.bump("auth_failures")
            self._json(
                401, {"ok": False, "code": "MG_AUTH_01", "message": "unauthorized"}
            )
            return
        body = self._read_body()
        if body is None:
            self.counters.bump("validation_failures")
            self._json(
                400, {"ok": False, "code": "MG_HTTP_00", "message": "JSON inválido"}
            )
            return
        if body.get("__too_large__"):
            self.counters.bump("validation_failures")
            self._json(
                413, {"ok": False, "code": "MG_HTTP_13", "message": "payload > 256 KiB"}
            )
            return
        ops = {
            "/save": (self.store.save, "saves_confirmed"),
            "/recall": (self.store.recall, "recalls"),
            "/get": (self.store.get, "gets"),
            "/list": (self.store.list, "lists"),
            "/delete": (self.store.delete, "deletes"),
            "/contradictions": (self.store.propose_contradiction, None),
            "/resolve": (self.store.resolve_contradiction, "contradictions_resolved"),
            "/reset": (lambda _p: self.store.reset_after_quarantine(), None),
        }
        op = ops.get(self.path)
        if op is None:
            self._json(404, {"ok": False, "code": "MG_HTTP_04", "message": "not_found"})
            return
        fn, counter = op
        t0 = time.perf_counter()
        try:
            result = fn(body)
        except GatewayError as e:
            if e.code == "MG_WRITE_01":
                self.counters.bump("saves_rejected")
            elif e.code == "MG_DB_02":
                self.counters.bump("db_busy")
            elif e.code == "MG_DB_03":
                self.counters.bump("db_corruption")
            elif e.code.startswith(("MG_VALID", "MG_SCOPE")):
                self.counters.bump("validation_failures")
            self._json(e.http, {"ok": False, "code": e.code, "message": e.message})
            return
        except Exception:  # noqa: BLE001 — fail-closed, sin fuga de detalles
            self._json(
                500, {"ok": False, "code": "MG_INT_99", "message": "error interno"}
            )
            return
        self.counters.record_latency((time.perf_counter() - t0) * 1000)
        if counter:
            self.counters.bump(counter)
        if self.path == "/save" and result.get("conflict"):
            self.counters.bump("contradictions_detected")
        self._json(200, result)


def build_server(
    store: MemoryStore,
    token: str,
    port: int = 0,
    activity: Activity | None = None,
) -> ThreadingHTTPServer:
    """Servidor bindeado SOLO a loopback; port=0 → efímero del SO."""
    handler = type(
        "BoundHandler",
        (GatewayHandler,),
        {
            "token": token,
            "store": store,
            "counters": Counters(),
            "activity": activity or Activity(),
        },
    )
    return ThreadingHTTPServer(("127.0.0.1", port), handler)
