"""Servidor HTTP del sidecar — stdlib, solo loopback, token obligatorio.

Seguridad (requisitos duros del Sprint 0):
- bind exclusivo a 127.0.0.1, puerto efímero (SO elige) publicado en
  runtime_dir/hormiguero.port;
- toda request (salvo /health) exige el header X-Nexum-Token igual al
  token 0600 de runtime_dir/hormiguero.token;
- nunca se loggea el token ni el texto del usuario;
- no hay red externa: el único egress es Ollama en 127.0.0.1 (opcional);
- no escribe archivos fuera del runtime dir; no ejecuta comandos.
"""

from __future__ import annotations

import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from . import __version__
from .auth import TOKEN_HEADER
from .classifier import classify, local_answer, model_available, public_model_name
from .envelope import build_task_envelope
from .watchdog import Activity

_START = time.monotonic()
_MAX_BODY = 64 * 1024  # 64 KiB: nadie clasifica más que eso


class Counters:
    """Observabilidad mínima (H-2 / G-2): contadores en memoria por instancia.

    Sin persistencia (el sidecar es stateless) y sin contenido sensible:
    solo números agregados y latencias. p50/p95 sobre una ventana acotada.
    """

    _WINDOW = 512

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self.classify_total = 0
        self.route_total = 0
        self.local_answers = 0
        self.escalations = 0
        self.bad_requests = 0
        self.unauthorized = 0
        self._latencies_ms: list[float] = []

    def record_latency(self, ms: float) -> None:
        with self._lock:
            self._latencies_ms.append(ms)
            if len(self._latencies_ms) > self._WINDOW:
                del self._latencies_ms[: -self._WINDOW]

    def bump(self, field: str) -> None:
        with self._lock:
            setattr(self, field, getattr(self, field) + 1)

    def snapshot(self) -> dict[str, object]:
        with self._lock:
            lat = sorted(self._latencies_ms)
            n = len(lat)

            def pct(p: int) -> float | None:
                if not lat:
                    return None
                return round(lat[min(n * p // 100, n - 1)], 2)

            return {
                "classify_total": self.classify_total,
                "route_total": self.route_total,
                "local_answers": self.local_answers,
                "escalations": self.escalations,
                "bad_requests": self.bad_requests,
                "unauthorized": self.unauthorized,
                "latency_window": n,
                "latency_p50_ms": pct(50),
                "latency_p95_ms": pct(95),
            }


class SidecarHandler(BaseHTTPRequestHandler):
    server_version = "nexum-hormiguero-sidecar"
    # Inyectados por build_server():
    token: str = ""
    debug: bool = False
    allow_shutdown: bool = False
    counters: Counters
    activity: Activity

    # ── Helpers ──────────────────────────────────────────────────────

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
        provided = self.headers.get(TOKEN_HEADER, "")
        # Comparación en tiempo constante (token nunca sale en la respuesta).
        import hmac

        return bool(self.token) and hmac.compare_digest(provided, self.token)

    def _read_body(self) -> dict[str, Any] | None:
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            return None
        if length <= 0 or length > _MAX_BODY:
            return None
        try:
            return json.loads(self.rfile.read(length).decode())
        except (json.JSONDecodeError, UnicodeDecodeError):
            return None

    def log_message(self, fmt: str, *args: Any) -> None:
        # Log seguro: método + ruta + status. Jamás cuerpos ni headers.
        pass

    # ── Rutas ────────────────────────────────────────────────────────

    def do_GET(self) -> None:  # noqa: N802 (firma de BaseHTTPRequestHandler)
        if self.path == "/health":
            self._json(
                200,
                {
                    "ok": True,
                    "service": "hormiguero-sidecar",
                    "version": __version__,
                    "uptime_ms": int((time.monotonic() - _START) * 1000),
                },
            )
            return
        if not self._authed():
            self.counters.bump("unauthorized")
            self._json(401, {"ok": False, "error": "unauthorized"})
            return
        if self.path == "/status":
            self._json(
                200,
                {
                    "ok": True,
                    "hormiguero_available": True,
                    "model_available": model_available(),
                    "model": public_model_name(self.debug),
                    "mode": "dev" if self.debug else "safe",
                    "capabilities": ["classify", "route"],
                    "counters": self.counters.snapshot(),
                },
            )
            return
        self._json(404, {"ok": False, "error": "not_found"})

    def do_POST(self) -> None:  # noqa: N802
        if not self._authed():
            self.counters.bump("unauthorized")
            self._json(401, {"ok": False, "error": "unauthorized"})
            return
        body = self._read_body()
        if self.path == "/classify":
            self.counters.bump("classify_total")
            if body is None or not isinstance(body.get("text"), str):
                self.counters.bump("bad_requests")
                self._json(400, {"ok": False, "error": "bad_request"})
                return
            t0 = time.perf_counter()
            c = classify(body["text"])
            self.counters.record_latency((time.perf_counter() - t0) * 1000)
            self._json(
                200,
                {
                    "ok": True,
                    "intent": c.intent,
                    "complexity": c.complexity,
                    "can_handle_local": c.can_handle_local,
                    "should_escalate": c.should_escalate,
                    "confidence": c.confidence,
                    "reason_public": c.reason_public,
                    "safety": {"sensitive": False, "requires_confirmation": False},
                },
            )
            return
        if self.path == "/route":
            self.counters.bump("route_total")
            if body is None or not isinstance(body.get("text"), str):
                self.counters.bump("bad_requests")
                self._json(400, {"ok": False, "error": "bad_request"})
                return
            _t0 = time.perf_counter()
            text = body["text"]
            locale = str(body.get("locale", "es"))
            source = str(body.get("source", "text"))
            budget = int(body.get("max_latency_ms", 400))
            c = classify(text, max_latency_ms=max(budget - 50, 100))
            if c.can_handle_local and not c.should_escalate:
                answer = local_answer(c.intent, locale)
                if answer is not None:
                    self.counters.bump("local_answers")
                    self.counters.record_latency((time.perf_counter() - _t0) * 1000)
                    self._json(
                        200,
                        {
                            "ok": True,
                            "decision": "local_answer",
                            "answer": answer,
                            "should_call_paid_ai": False,
                            "task_envelope": None,
                            "confidence": c.confidence,
                        },
                    )
                    return
            envelope = build_task_envelope(
                text,
                source,
                escalation_reason=c.reason_public,
                confidence=c.confidence,
            )
            self.counters.bump("escalations")
            self.counters.record_latency((time.perf_counter() - _t0) * 1000)
            self._json(
                200,
                {
                    "ok": True,
                    "decision": "needs_paid_ai",
                    "answer": None,
                    "should_call_paid_ai": True,
                    "task_envelope": envelope,
                    "confidence": c.confidence,
                },
            )
            return
        if self.path == "/plan":
            # OMEGA Fase 7: Planner→Critic→Refiner→Validator. Vista pública
            # segura (decisión + códigos), jamás reasoning interno.
            if body is None or not isinstance(body.get("text"), str):
                self.counters.bump("bad_requests")
                self._json(400, {"ok": False, "error": "bad_request"})
                return
            from .planning import LOW, run_pipeline
            from .planning.profiles import PROFILES

            profile = PROFILES.get(str(body.get("profile", "low")), LOW)
            task_class = str(body.get("task_class", "generic"))
            outcome = run_pipeline(body["text"], task_class, profile)
            self._json(200, {"ok": True, **outcome.public_dict()})
            return
        if self.path == "/shutdown":
            if not self.allow_shutdown:
                self._json(403, {"ok": False, "error": "disabled"})
                return
            self._json(200, {"ok": True})
            # Apagado limpio desde otro thread (no bloquear el handler).
            import threading

            threading.Thread(target=self.server.shutdown, daemon=True).start()
            return
        self._json(404, {"ok": False, "error": "not_found"})


def build_server(
    token: str,
    debug: bool = False,
    allow_shutdown: bool = True,
    port: int = 0,
    activity: Activity | None = None,
) -> ThreadingHTTPServer:
    """Servidor bindeado SOLO a loopback; port=0 → efímero del SO."""
    handler = type(
        "BoundHandler",
        (SidecarHandler,),
        {
            "token": token,
            "debug": debug,
            "allow_shutdown": allow_shutdown,
            "counters": Counters(),
            "activity": activity or Activity(),
        },
    )
    return ThreadingHTTPServer(("127.0.0.1", port), handler)
