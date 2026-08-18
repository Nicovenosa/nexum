"""Singleton y ciclo de vida del sidecar de memoria (CHANGE-RUNTIME-001).

Idéntico contrato que el sidecar Hormiguero (sin causa demostrable para
divergir): flock sostenido de por vida (lock tomado ⟺ instancia viva),
exit 4 reuso sano, exit 5 instancia colgada, limpieza de metadata stale
solo con el lock en mano, kill únicamente con cmdline validado.
"""

from __future__ import annotations

import fcntl
import json
import urllib.error
import urllib.request
from pathlib import Path
from typing import IO

from .auth import LOCK_FILE, PID_FILE, PORT_FILE, SERVICE_NAME, TOKEN_FILE, runtime_dir

EXIT_FLAG_OFF = 3
EXIT_ALREADY_RUNNING = 4
EXIT_UNHEALTHY_INSTANCE = 5

_HEALTH_TIMEOUT_S = 1.0  # contrato: health responde < 1 s


def acquire_instance_lock(base: Path | None = None) -> IO[bytes] | None:
    """Devuelve el file object del lock (mantener vivo hasta la muerte del
    proceso) o None si otra instancia lo sostiene."""
    base = base or runtime_dir()
    handle = open(base / LOCK_FILE, "wb")  # noqa: SIM115 (vive lo que el proceso)
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        handle.close()
        return None
    return handle


def clean_stale_metadata(base: Path | None = None) -> None:
    """Solo con el lock en mano: pid/port/token preexistentes son basura de
    un dueño muerto."""
    base = base or runtime_dir()
    for name in (PID_FILE, PORT_FILE, TOKEN_FILE):
        try:
            (base / name).unlink(missing_ok=True)
        except OSError:
            pass


def probe_existing_instance(base: Path | None = None) -> bool:
    """¿La instancia que sostiene el lock responde /health con la identidad
    correcta? Nunca lanza: cualquier fallo ⇒ False."""
    base = base or runtime_dir()
    try:
        port = int((base / PORT_FILE).read_text(encoding="ascii").strip())
    except (OSError, ValueError):
        return False
    try:
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/health", timeout=_HEALTH_TIMEOUT_S
        ) as resp:
            payload = json.loads(resp.read())
    except (urllib.error.URLError, OSError, ValueError):
        return False
    return bool(payload.get("ok")) and payload.get("service") == SERVICE_NAME


def owner_pid_is_sidecar(base: Path | None = None) -> int | None:
    """PID del pidfile validado por cmdline. Jamás matar a ciegas."""
    base = base or runtime_dir()
    try:
        pid = int((base / PID_FILE).read_text(encoding="ascii").strip())
    except (OSError, ValueError):
        return None
    try:
        cmdline = Path(f"/proc/{pid}/cmdline").read_bytes()
    except OSError:
        return None
    return pid if b"nexum_memory_gateway" in cmdline else None
