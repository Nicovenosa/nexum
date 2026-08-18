"""Nexum Bridge Supervisor — autostart y estado de CLIProxyAPI (Sprint C).

Corre UNA vez en el bootstrap de Nexum (lo invoca el launcher antes de
regenerar el catálogo) y a demanda desde /provedor (shortcut `r` y el flujo
explícito "conectar puente").

Uso CLI (siempre imprime UNA línea JSON en stdout):

    bridge_supervisor.py --ensure
        → {"status": "not_installed"|"installed_not_running"|"running"|
                     "start_failed",
           "installed": bool, "running": bool, "started_by_us": bool,
           "management": "available"|"locked"|"unavailable"|null,
           "auth_files": [{"provider","status","email","last_refresh"}]|null,
           "warnings": [..], "reason": str|null}

    bridge_supervisor.py --connect <nexum_provider_id>
        → {"ok": true, "url": "...", "state": "..."} | {"ok": false, "message"}

    bridge_supervisor.py --poll <state>
        → {"status": "wait"|"ok"|"error"}

Seguridad (spec §4.4):
  - Solo 127.0.0.1. Si la config del servicio bindea otra cosa (host vacío
    o 0.0.0.0 = todas las interfaces), se agrega un warning — NO se edita
    la config.
   - La management key se lee de CLIPROXYAPI_MANAGEMENT_KEY (env). Jamás se
     imprime ni loguea.
  - Timeout global de 3s para el ensure (poll de 300ms tras el start).
  - No hace OAuth automático: --connect/--poll son acciones explícitas
    del usuario desde /provedor.

Stdlib only.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import asdict
from pathlib import Path
from typing import Any

# Import dual: funciona tanto cuando se carga como parte del paquete
# `nexum_providers` (tests: `from nexum_providers import bridge_supervisor`)
# como cuando se ejecuta como script standalone (Rust:
# `python3 src/nexum_providers/bridge_supervisor.py`), donde el directorio
# del script está en sys.path y los módulos hermanos son top-level.
try:
    from nexum_providers.oauth_callback_discovery import parse_callback
except ImportError:  # modo script standalone
    from oauth_callback_discovery import parse_callback

HOST = "127.0.0.1"
PORT = 8317
BASE_URL = f"http://{HOST}:{PORT}"
MGMT_URL = f"{BASE_URL}/v0/management"
SYSTEMD_UNIT = "cli-proxy-api"
BINARY_NAME = "cli-proxy-api"
# Paths relevados en la auditoría (3.1) además del PATH.
KNOWN_BINARY_PATHS = (
    "~/.local/bin/cli-proxy-api",
    "/usr/bin/cli-proxy-api",
    "/usr/local/bin/cli-proxy-api",
)
CONFIG_PATH = Path("~/.cli-proxy-api/config.yaml").expanduser()

ENSURE_TIMEOUT_SECS = 3.0
POLL_INTERVAL_SECS = 0.3
HTTP_TIMEOUT_SECS = 2.0

# Mapa provider Nexum → provider id de CLIProxyAPI para el flujo OAuth
# (auditado contra el binario 7.2.50: anthropic/codex/antigravity;
# Google/Gemini se puentea vía el flujo Antigravity — no hay gemini-auth-url).
OAUTH_PROVIDER_MAP = {
    "claude_code": "anthropic",
    "codex_cli": "codex",
    "gemini_cli": "antigravity",
}


# Archivos de secretos donde puede vivir una credencial del puente, en orden.
MANAGEMENT_KEY_FILES: tuple[str, ...] = (
    "~/.config/nexum/secrets.env",
    "~/.nexum/secrets.env",
)


def _key_from_env_file(name: str) -> str | None:
    """Lee `name` de los archivos de secretos declarados. Nunca loguea el valor.

    Existe porque la management key vive en disco y sólo se la buscaba en el
    entorno: durante el Sprint 1 eso se diagnosticó como «Management API
    deshabilitada», cuando en realidad la credencial estaba a un archivo de
    distancia. Mismo patrón que la api-key del puente.
    """
    for raw in MANAGEMENT_KEY_FILES:
        path = Path(raw).expanduser()
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except OSError:
            continue
        for line in lines:
            text = line.strip()
            if not text or text.startswith("#"):
                continue
            match = re.match(
                r"^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$", text
            )
            if match and match.group(1) == name:
                value = match.group(2).strip().strip('"').strip("'")
                if value:
                    return value
    return None


def _management_key() -> str | None:
    """Entorno primero, después los archivos de secretos declarados."""
    value = os.environ.get("CLIPROXYAPI_MANAGEMENT_KEY", "").strip()
    return value or _key_from_env_file("CLIPROXYAPI_MANAGEMENT_KEY")


def find_binary() -> str | None:
    found = shutil.which(BINARY_NAME)
    if found:
        return found
    for raw in KNOWN_BINARY_PATHS:
        p = Path(raw).expanduser()
        if p.is_file() and os.access(p, os.X_OK):
            return str(p)
    return None


def port_open(timeout: float = 0.5) -> bool:
    try:
        with socket.create_connection((HOST, PORT), timeout=timeout):
            return True
    except OSError:
        return False


def _config_bind_warnings() -> list[str]:
    """Solo mira la línea `host:` del config (nunca la secret-key)."""
    try:
        for line in CONFIG_PATH.read_text(encoding="utf-8").splitlines():
            s = line.strip()
            if s.startswith("host:"):
                value = s.split(":", 1)[1].strip().strip('"').strip("'")
                if value in ("", "0.0.0.0", "::"):
                    return [
                        "CLIProxyAPI está configurado para bindear TODAS las "
                        f"interfaces (host={value!r}). Recomendado: 127.0.0.1. "
                        "No se modificó la config."
                    ]
                return []
    except OSError:
        pass
    return []


def _mgmt_get(path: str, key: str | None) -> tuple[int | None, Any]:
    """GET a la Management API local. Devuelve (status_code, json|None)."""
    url = f"{MGMT_URL}{path}"
    if not url.startswith(f"http://{HOST}"):
        return None, None
    headers = {"Accept": "application/json"}
    if key:
        headers["Authorization"] = f"Bearer {key}"
    req = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=HTTP_TIMEOUT_SECS) as resp:  # noqa: S310
            raw = resp.read().decode("utf-8", errors="replace")
            try:
                return resp.status, json.loads(raw)
            except json.JSONDecodeError:
                return resp.status, None
    except urllib.error.HTTPError as exc:
        return exc.code, None
    except (urllib.error.URLError, TimeoutError, OSError):
        return None, None


def _safe_auth_files(payload: Any) -> list[dict[str, Any]]:
    """Extrae SOLO campos seguros de /auth-files (jamás tokens)."""
    files: Any = []
    if isinstance(payload, dict):
        files = (
            payload.get("files")
            or payload.get("auth_files")
            or payload.get("data")
            or []
        )
    elif isinstance(payload, list):
        files = payload
    out = []
    for f in files:
        if isinstance(f, dict):
            out.append(
                {
                    "provider": f.get("provider"),
                    "status": f.get("status"),
                    "disabled": bool(f.get("disabled") or f.get("unavailable")),
                    "email": f.get("email"),
                    "last_refresh": f.get("last_refresh"),
                }
            )
    return out


def _query_management(key: str | None) -> tuple[str, list[dict[str, Any]] | None]:
    """→ (management_status, auth_files|None). 401 = locked (pero vivo)."""
    code, payload = _mgmt_get("/auth-files", key)
    if code == 200:
        return "available", _safe_auth_files(payload)
    if code in (401, 403):
        return "locked", None
    return "unavailable", None


def _try_start() -> tuple[bool, str | None]:
    """Autostart vía systemctl --user (unit relevada en la auditoría 3.2).

    Sin fallback a binario directo (decisión 3.6: un proceso huérfano sin
    supervisión es peor que un StartFailed claro).
    """
    if not shutil.which("systemctl"):
        return False, "systemctl no disponible (sin systemd user)"
    try:
        proc = subprocess.run(
            ["systemctl", "--user", "start", SYSTEMD_UNIT],
            capture_output=True,
            text=True,
            timeout=ENSURE_TIMEOUT_SECS,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return False, f"systemctl falló: {exc}"
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout or "").strip()[:160]
        return False, f"systemctl --user start {SYSTEMD_UNIT}: {detail or 'error'}"
    return True, None


def ensure_running() -> dict[str, Any]:
    """La función del spec §4.2: detectar → arrancar → esperar → clasificar."""
    deadline = time.monotonic() + ENSURE_TIMEOUT_SECS
    binary = find_binary()
    result: dict[str, Any] = {
        "status": "not_installed",
        "installed": binary is not None,
        "binary_path": binary,
        "running": False,
        "started_by_us": False,
        "management": None,
        "auth_files": None,
        "warnings": [],
        "reason": None,
    }
    if binary is None:
        result["reason"] = (
            "No se encontró el binario cli-proxy-api (PATH ni rutas conocidas)."
        )
        return result

    running = port_open()
    if not running:
        ok, err = _try_start()
        if not ok:
            result["status"] = "start_failed"
            result["reason"] = err
            return result
        result["started_by_us"] = True
        # Poll cada 300ms hasta el deadline global de 3s.
        while time.monotonic() < deadline:
            if port_open(timeout=0.3):
                running = True
                break
            time.sleep(POLL_INTERVAL_SECS)
        if not running:
            result["status"] = "start_failed"
            result["reason"] = (
                f"El servicio no abrió {HOST}:{PORT} dentro de los "
                f"{ENSURE_TIMEOUT_SECS:.0f}s de timeout."
            )
            return result

    result["running"] = True
    management, auth_files = _query_management(_management_key())
    result["status"] = "running"
    result["management"] = management
    result["auth_files"] = auth_files
    result["warnings"] = _config_bind_warnings()
    return result


# ─── Conectar puente (OAuth EXPLÍCITO del usuario, spec §1.2/§8.2) ───────────


def connect_url(nexum_provider_id: str) -> dict[str, Any]:
    cpid = OAUTH_PROVIDER_MAP.get(nexum_provider_id.strip().replace("-", "_"))
    if not cpid:
        return {
            "ok": False,
            "message": f"Provider sin flujo de puente: {nexum_provider_id}",
        }
    if not port_open():
        return {"ok": False, "message": "CLIProxyAPI no está corriendo."}
    # is_webui=true es OBLIGATORIO (auditado en el source 7.2.50,
    # internal/api/handlers/management/auth_files.go): sin ese query param el
    # handler de {provider}-auth-url NO arranca el callbackForwarder local
    # (listener en 1455/54545/51121) y el navegador muere con
    # ERR_CONNECTION_REFUSED. Con is_webui=true, CLIProxyAPI levanta el
    # forwarder, redirige el callback a 127.0.0.1:8317/{provider}/callback,
    # intercambia el code internamente y get-auth-status pasa a "ok".
    code, payload = _mgmt_get(f"/{cpid}-auth-url?is_webui=true", _management_key())
    if code in (401, 403):
        return {
            "ok": False,
            "message": "Management API bloqueada (falta management key).",
        }
    if code != 200 or not isinstance(payload, dict):
        return {
            "ok": False,
            "message": f"No se pudo iniciar el flujo OAuth (HTTP {code}).",
        }
    url = payload.get("url")
    state = payload.get("state")
    if not (isinstance(url, str) and isinstance(state, str)):
        return {"ok": False, "message": "Respuesta OAuth sin url/state."}
    # Descubrir el callback OAuth esperado desde la auth_url (sin red, stdlib).
    # Solo host/port/path sanitizados — nunca code/token/state (el parser los
    # descarta explícitamente). Esto le da a Rust el callback info sin un
    # round-trip extra, para que pueda diagnosticar el listener.
    callback = asdict(parse_callback(url))
    return {
        "ok": True,
        "url": url,
        "state": state,
        "cliproxy_provider": cpid,
        "callback": callback,
    }


def poll_state(state: str) -> dict[str, Any]:
    code, payload = _mgmt_get(f"/get-auth-status?state={state}", _management_key())
    if code == 200 and isinstance(payload, dict):
        s = payload.get("status")
        if isinstance(s, str):
            return {"status": s}
    return {"status": "error"}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Nexum Bridge Supervisor (Sprint C).")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument(
        "--ensure", action="store_true", help="Detectar/arrancar/clasificar."
    )
    group.add_argument(
        "--connect", metavar="PROVIDER_ID", help="Iniciar OAuth explícito."
    )
    group.add_argument("--poll", metavar="STATE", help="Poll del flujo OAuth.")
    args = parser.parse_args(argv)

    if args.ensure:
        out = ensure_running()
    elif args.connect:
        out = connect_url(args.connect)
    else:
        out = poll_state(args.poll)
    print(json.dumps(out, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
