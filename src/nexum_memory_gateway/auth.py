"""Token local + runtime dir + ubicación de la DB del MemoryGateway.

Mismo patrón que el sidecar Hormiguero (CHANGE-RUNTIME-001): runtime dir
0700, metadata 0600, token jamás impreso ni loggeado, regenerado en cada
arranque (R-5 de SPEC-MEMORY-001; rotación en caliente: post-v0.1).
La DB del usuario vive en XDG_DATA_HOME (persistente), NO en el runtime
dir (volátil).
"""

from __future__ import annotations

import os
import secrets
import stat
from pathlib import Path

PORT_FILE = "memory.port"
TOKEN_FILE = "memory.token"
PID_FILE = "memory.pid"
LOCK_FILE = "memory.lock"
TOKEN_HEADER = "X-Nexum-Memory-Token"
SERVICE_NAME = "memory-gateway"


def runtime_dir() -> Path:
    """Directorio de runtime (0700). Prioridad: env explícita > XDG > /tmp."""
    explicit = os.environ.get("NEXUM_MEMORY_RUNTIME_DIR")
    if explicit:
        base = Path(explicit)
    else:
        xdg = os.environ.get("XDG_RUNTIME_DIR")
        if xdg:
            base = Path(xdg) / "nexum-memory"
        else:
            base = Path(f"/tmp/nexum-memory-{os.getuid()}")
    base.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(base, 0o700)
    return base


def db_path() -> str:
    """DB persistente del usuario. Prioridad: env > XDG_DATA_HOME > ~/.local/share."""
    explicit = os.environ.get("NEXUM_MEMORY_DB")
    if explicit:
        Path(explicit).parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        return explicit
    data = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    base = Path(data) / "nexum" / "memory"
    base.mkdir(mode=0o700, parents=True, exist_ok=True)
    return str(base / "memory.sqlite3")


def generate_token() -> str:
    """Genera y persiste el token de sesión (0600). NUNCA se imprime/loggea."""
    token = secrets.token_hex(32)
    path = runtime_dir() / TOKEN_FILE
    path.write_text(token, encoding="ascii")
    os.chmod(path, stat.S_IRUSR | stat.S_IWUSR)  # 0600
    return token


def write_port(port: int) -> None:
    path = runtime_dir() / PORT_FILE
    path.write_text(str(port), encoding="ascii")
    os.chmod(path, stat.S_IRUSR | stat.S_IWUSR)


def write_pid() -> None:
    path = runtime_dir() / PID_FILE
    path.write_text(str(os.getpid()), encoding="ascii")
    os.chmod(path, stat.S_IRUSR | stat.S_IWUSR)


def cleanup_files() -> None:
    for name in (PORT_FILE, TOKEN_FILE, PID_FILE):
        try:
            (runtime_dir() / name).unlink(missing_ok=True)
        except OSError:
            pass
