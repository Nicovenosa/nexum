"""Singleton y ciclo de vida del sidecar (CHANGE-RUNTIME-001).

Contrato: un runtime dir = a lo sumo UNA instancia viva. La exclusión es un
flock(LOCK_EX|LOCK_NB) sobre hormiguero.lock sostenido de por vida; el kernel
lo libera ante cualquier muerte (incluido kill -9), así que *lock tomado ⟺
instancia viva* — sin razas ni PIDs reciclados. Exit codes normalizados:
0 shutdown limpio · 4 instancia sana ya corriendo (reuse) · 5 lock tomado
pero la instancia no responde el health check.
"""

from __future__ import annotations

import fcntl
import json
import urllib.error
import urllib.request
from pathlib import Path
from typing import IO

from .auth import PID_FILE, PORT_FILE, TOKEN_FILE, runtime_dir

LOCK_FILE = "hormiguero.lock"

EXIT_ALREADY_RUNNING = 4
EXIT_UNHEALTHY_INSTANCE = 5

_HEALTH_TIMEOUT_S = 1.0  # presupuesto del contrato: health responde < 1 s


def acquire_instance_lock(base: Path | None = None) -> IO[bytes] | None:
    """Intenta tomar el lock de instancia. Devuelve el file object (mantener
    vivo hasta la muerte del proceso) o None si otra instancia lo sostiene.
    """
    base = base or runtime_dir()
    handle = open(base / LOCK_FILE, "wb")  # noqa: SIM115 (vive lo que el proceso)
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        handle.close()
        return None
    return handle


def clean_stale_metadata(base: Path | None = None) -> None:
    """Con el lock en mano, pid/port/token preexistentes son basura de un
    dueño muerto (crash sin cleanup): eliminarlos antes de escribir los
    propios. Jamás se llama sin poseer el lock.
    """
    base = base or runtime_dir()
    for name in (PID_FILE, PORT_FILE, TOKEN_FILE):
        try:
            (base / name).unlink(missing_ok=True)
        except OSError:
            pass


def probe_existing_instance(base: Path | None = None) -> bool:
    """¿La instancia que sostiene el lock responde /health como sidecar?

    Sin auth (health es público) y con identidad verificada: debe declararse
    ``hormiguero-sidecar``. Nunca lanza: cualquier fallo ⇒ False.
    """
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
    return bool(payload.get("ok")) and payload.get("service") == "hormiguero-sidecar"


def owner_pid_is_sidecar(base: Path | None = None) -> int | None:
    """PID del pidfile validado por cmdline (nunca matar a ciegas).

    Devuelve el PID solo si /proc/<pid>/cmdline contiene el nombre del
    módulo; en cualquier otro caso None.
    """
    base = base or runtime_dir()
    try:
        pid = int((base / PID_FILE).read_text(encoding="ascii").strip())
    except (OSError, ValueError):
        return None
    try:
        cmdline = Path(f"/proc/{pid}/cmdline").read_bytes()
    except OSError:
        return None
    return pid if b"nexum_hormiguero_sidecar" in cmdline else None
