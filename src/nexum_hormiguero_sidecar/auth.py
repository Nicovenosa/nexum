"""Token local + runtime dir del sidecar.

El launcher (scripts/nexum) exporta NEXUM_HORMIGUERO_RUNTIME_DIR para que
Rust y Python coincidan siempre en el mismo directorio. Fallbacks idénticos
en ambos lados: $XDG_RUNTIME_DIR/nexum → /tmp/nexum-$UID (0700).
"""

from __future__ import annotations

import os
import secrets
import stat
from pathlib import Path

PORT_FILE = "hormiguero.port"
TOKEN_FILE = "hormiguero.token"
PID_FILE = "hormiguero.pid"
TOKEN_HEADER = "X-Nexum-Token"


def runtime_dir() -> Path:
    """Directorio de runtime (0700). Prioridad: env explícita > XDG > /tmp."""
    explicit = os.environ.get("NEXUM_HORMIGUERO_RUNTIME_DIR")
    if explicit:
        base = Path(explicit)
    else:
        xdg = os.environ.get("XDG_RUNTIME_DIR")
        if xdg:
            base = Path(xdg) / "nexum"
        else:
            base = Path(f"/tmp/nexum-{os.getuid()}")
    base.mkdir(mode=0o700, parents=True, exist_ok=True)
    # Endurecer por si existía con otros permisos.
    os.chmod(base, 0o700)
    return base


def generate_token() -> str:
    """Genera y persiste el token de sesión con permisos 0600.

    El token NUNCA se imprime ni se loggea.
    """
    token = secrets.token_hex(32)
    path = runtime_dir() / TOKEN_FILE
    # O_EXCL no: el launcher puede relanzar; escribir atómico y chmod.
    path.write_text(token, encoding="ascii")
    os.chmod(path, stat.S_IRUSR | stat.S_IWUSR)  # 0600
    return token


def read_token() -> str | None:
    path = runtime_dir() / TOKEN_FILE
    try:
        return path.read_text(encoding="ascii").strip() or None
    except OSError:
        return None


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
