"""Watchdog de ciclo de vida del sidecar (OMEGA Fase 5 — cierra B-1).

Problema real: el sidecar sobrevivía a su mundo. Evidencia forense (2026-07-20):
una instancia de benchmark vivió 2 días (runtime dir /tmp/horm-bench-*) y una
del dogfood 18 horas, ambas reparented a systemd tras morir su lanzador, que
nunca envió SIGTERM.

Dos guardias complementarias, ambas stdlib y fail-safe:

1. Parent-PID watch (spawns supervisados): si el lanzador exporta
   ``NEXUM_SIDECAR_PARENT_PID``, el watchdog verifica cada ~2s que ese PID siga
   vivo. Muerto el padre ⇒ shutdown limpio. No usa PPID (el launcher bash
   lanza con nohup y el PPID inicial muere al instante por diseño).

2. Idle-TTL (default, cubre cualquier spawn): sin NINGUNA request HTTP durante
   ``NEXUM_SIDECAR_IDLE_TTL_SECS`` (default 1800s, 0 = deshabilitado) ⇒
   shutdown limpio. Un runtime vivo que use el sidecar lo mantiene despierto;
   un residuo huérfano muere solo. El launcher ya sabe relanzar (singleton
   CHANGE-RUNTIME-001: exit 4 = reuse de instancia sana).

El shutdown es el mismo camino que SIGTERM: ``server.shutdown()`` desde este
thread (nunca desde un signal handler) ⇒ serve_forever retorna ⇒ el ``finally``
de __main__ limpia metadata y suelta el lock.
"""

from __future__ import annotations

import os
import sys
import threading
import time
from typing import Protocol


class _Shutdownable(Protocol):
    def shutdown(self) -> None: ...  # pragma: no cover - protocolo


class Activity:
    """Marca de última request atendida (thread-safe, monotonic)."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._last = time.monotonic()

    def touch(self) -> None:
        with self._lock:
            self._last = time.monotonic()

    def idle_secs(self) -> float:
        with self._lock:
            return time.monotonic() - self._last


def parent_pid_from_env() -> int | None:
    """PID del proceso supervisor declarado por el lanzador (o None)."""
    raw = os.environ.get("NEXUM_SIDECAR_PARENT_PID", "").strip()
    if not raw:
        return None
    try:
        pid = int(raw)
    except ValueError:
        return None
    return pid if pid > 1 else None


def idle_ttl_from_env(default_secs: float = 1800.0) -> float:
    """TTL de inactividad; 0 (o inválido negativo) = deshabilitado."""
    raw = os.environ.get("NEXUM_SIDECAR_IDLE_TTL_SECS", "").strip()
    if not raw:
        return default_secs
    try:
        ttl = float(raw)
    except ValueError:
        return default_secs
    return max(ttl, 0.0)


def _pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        # Existe pero es de otro usuario: contarlo como vivo (fail-safe).
        return True
    except OSError:
        return True
    return True


def start(
    server: _Shutdownable,
    activity: Activity,
    *,
    parent_pid: int | None,
    idle_ttl_secs: float,
    poll_secs: float = 2.0,
    service_name: str = "hormiguero-sidecar",
) -> threading.Thread:
    """Arranca el watchdog como thread daemon y lo devuelve (para tests)."""

    def _loop() -> None:
        while True:
            time.sleep(poll_secs)
            if parent_pid is not None and not _pid_alive(parent_pid):
                print(
                    f"[{service_name}] watchdog: el proceso supervisor "
                    f"(pid {parent_pid}) murió — shutdown limpio",
                    file=sys.stderr,
                )
                server.shutdown()
                return
            if idle_ttl_secs > 0 and activity.idle_secs() > idle_ttl_secs:
                print(
                    f"[{service_name}] watchdog: sin requests hace más de "
                    f"{idle_ttl_secs:.0f}s — shutdown limpio por inactividad",
                    file=sys.stderr,
                )
                server.shutdown()
                return

    thread = threading.Thread(target=_loop, daemon=True, name="lifecycle-watchdog")
    thread.start()
    return thread
