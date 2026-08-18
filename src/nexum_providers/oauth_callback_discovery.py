"""Nexum OAuth Callback Discovery — descubrimiento dinámico del callback OAuth.

El puente CLIProxyAPI inicia OAuth devolviendo una auth_url. El navegador
completa el login y redirige a un callback LOCAL (ej. localhost:1455) que
CLIProxyAPI debe estar escuchando. Si no lo está, el login cae con
ERR_CONNECTION_REFUSED.

Este módulo:
  1. PARSEA la auth_url para descubrir el callback esperado (host/port/path)
     sin hardcodear puertos. Fuentes (en orden de preferencia):
       a) query param `redirect_uri` (o `redirect_uri_encoded`)
       b) inferencia desde el netloc si la auth_url apunta a localhost
       c) `unknown`
  2. DIAGNOSTICA si el puerto del callback está siendo escuchado (IPv4/IPv6).
  3. WATCH cross-platform de puertos en LISTEN (ss/netstat/lsof).

Seguridad:
  - La auth_url NO es un secret (es la página de autorización del provider).
    Aun así, este módulo NUNCA emite code/token/state completos: solo host,
    port y path sanitizados del callback.
  - Stdlib only. Sync (invocado como subprocess por el TUI).

Contrato CLI (JSON sobre stdin/stdout):

    stdin:  {"url": "<auth_url>", "state": "<state>"?}   (state se ignora)
    stdout: {"ok": true,
             "callback": {"host", "port", "path", "source"},
             "listener": {"detected", "ipv4", "ipv6", "conflict", "tool"}}
          | {"ok": false, "message": "..."}
"""

from __future__ import annotations

import argparse
import json
import shutil
import socket
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, unquote, urlparse

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))


# ─── Callback info ────────────────────────────────────────────────────────────

# Hosts que consideramos "localhost" para inferencia de callback.
_LOCAL_HOSTS = frozenset({"localhost", "127.0.0.1", "::1", "[::1]", "0.0.0.0"})


@dataclass
class CallbackInfo:
    """Callback OAuth esperado, sanitizado (solo host/port/path).

    source:
      "parsed_from_redirect_uri" — había un redirect_uri query param.
      "inferred"                 — inferido desde el netloc de la auth_url.
      "unknown"                  — no se pudo determinar.
    """

    host: str | None
    port: int | None
    path: str | None
    source: str


def _parse_host_port(netloc: str) -> tuple[str | None, int | None]:
    """Extrae (host, port) de un netloc tipo 'localhost:1455' o '[::1]:5112'.

    Devuelve (None, None) si no hay puerto (no es un callback local útil).
    """
    if not netloc:
        return None, None
    # Caso IPv6 [::1]:port
    if netloc.startswith("["):
        end = netloc.find("]")
        if end == -1:
            return None, None
        host = netloc[: end + 1]
        rest = netloc[end + 1 :]
        if rest.startswith(":"):
            try:
                return host, int(rest[1:])
            except ValueError:
                return host, None
        return host, None
    # IPv4 / hostname
    if ":" in netloc:
        host_s, _, port_s = netloc.rpartition(":")
        try:
            return host_s, int(port_s)
        except ValueError:
            return host_s, None
    return netloc, None


def parse_callback(auth_url: str) -> CallbackInfo:
    """Descubre el callback OAuth esperado desde la auth_url.

    No lanza; ante cualquier ambigüedad devuelve source="unknown".
    """
    if not auth_url or not isinstance(auth_url, str):
        return CallbackInfo(None, None, None, "unknown")

    try:
        parsed = urlparse(auth_url)
    except (ValueError, TypeError):
        return CallbackInfo(None, None, None, "unknown")

    qs = parse_qs(parsed.query, keep_blank_values=True)

    # (a) redirect_uri query param (puede venir URL-encoded).
    for key in ("redirect_uri", "redirect_uri_encoded"):
        if key in qs and qs[key]:
            raw = qs[key][0]
            candidate = unquote(raw)
            return _callback_from_url(candidate, "parsed_from_redirect_uri")

    # (b) Inferencia: si la auth_url MISMA apunta a un localhost con puerto,
    # ese es probablemente el callback (algunos providers redirigen directo).
    host, port = _parse_host_port(parsed.netloc)
    if host is not None and port is not None:
        bare_host = host.strip("[]").lower()
        if bare_host in _LOCAL_HOSTS:
            path = parsed.path or "/"
            return CallbackInfo(bare_host, port, path, "inferred")

    # (c) No se pudo determinar.
    return CallbackInfo(None, None, None, "unknown")


def _callback_from_url(url: str, source: str) -> CallbackInfo:
    """Construye CallbackInfo desde una URL de callback explícita."""
    try:
        parsed = urlparse(url)
    except (ValueError, TypeError):
        return CallbackInfo(None, None, None, "unknown")
    host, port = _parse_host_port(parsed.netloc)
    bare_host = host.strip("[]").lower() if host else None
    path = parsed.path or "/" if (host and port) else None
    if not host or not port:
        return CallbackInfo(None, None, None, "unknown")
    return CallbackInfo(bare_host, port, path, source)


# ─── Listener diagnostics ────────────────────────────────────────────────────

LISTEN_PROBE_TIMEOUT = 0.5
PORT_WATCH_TIMEOUT = 5.0


@dataclass
class ListenerDiag:
    """Diagnóstico de si el puerto del callback está escuchando."""

    detected: bool
    ipv4: bool
    ipv6: bool
    conflict: bool
    tool: str  # "tcp_probe" | "ss" | "netstat" | "lsof" | "none"


def _tcp_probe(host: str, port: int, timeout: float = LISTEN_PROBE_TIMEOUT) -> bool:
    """True si hay algo escuchando en host:port (TCP connect, read-only)."""
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def diagnose_listener(host: str | None, port: int | None) -> ListenerDiag:
    """Diagnostica IPv4/IPv6 listening del puerto del callback.

    Usa TCP connect probe (siempre) + port watcher (ss/netstat/lsof) para
    detectar conflictos (múltiples procesos en el mismo puerto).
    """
    if not host or not port:
        return ListenerDiag(False, False, False, False, "none")

    # Normalizar host: localhost → probar ambas familias.
    h = host.strip("[]").lower()
    ipv4_targets: list[str] = []
    ipv6_targets: list[str] = []
    if h in ("localhost", "127.0.0.1", "0.0.0.0"):
        ipv4_targets = ["127.0.0.1"]
        ipv6_targets = ["::1"]
    elif h == "::1":
        ipv6_targets = ["::1"]
    else:
        ipv4_targets = [h]

    ipv4 = any(_tcp_probe(t, port) for t in ipv4_targets)
    ipv6 = any(_tcp_probe(t, port) for t in ipv6_targets)
    detected = ipv4 or ipv6

    # Conflict detection via port watcher (best-effort; no fatal si falla).
    procs = watch_port_listening(port)
    conflict = len(procs) > 1
    tool = "tcp_probe" if not procs else _detect_watch_tool()

    return ListenerDiag(detected, ipv4, ipv6, conflict, tool)


# ─── Cross-platform port watcher ──────────────────────────────────────────────


def _detect_watch_tool() -> str:
    """Devuelve qué herramienta de watch está disponible en este OS."""
    for tool, bins in (
        ("ss", ("ss",)),
        ("lsof", ("lsof",)),
        ("netstat", ("netstat",)),
    ):
        if any(shutil.which(b) for b in bins):
            return tool
    return "none"


def watch_port_listening(port: int) -> list[dict[str, str]]:
    """Lista procesos que están escuchando en `port` (cross-platform).

    Devuelve [{"proto","addr","port","pid?"}]. Vacío si la herramienta no
    está disponible o el puerto no aparece. Best-effort: nunca lanza.
    """
    if port <= 0:
        return []
    plat = sys.platform
    if plat == "linux":
        text = _run_port_cmd(["ss", "-ltnp"])
        return _parse_ss_linux(text, port) if text else []
    if plat == "darwin":
        text = _run_port_cmd(["lsof", "-iTCP", "-sTCP:LISTEN", "-P", "-n"])
        return _parse_lsof_macos(text, port) if text else []
    if plat == "win32":
        text = _run_port_cmd(["netstat", "-ano"])
        return _parse_netstat_windows(text, port) if text else []
    # OS no soportado: sin watcher (el TCP probe igual funciona).
    return []


def _run_port_cmd(argv: list[str]) -> str:
    """Ejecuta un comando de diagnóstico de puertos de forma segura."""
    if not shutil.which(argv[0]):
        return ""
    try:
        proc = subprocess.run(  # noqa: S603 - argv fijo, binario del sistema
            argv,
            capture_output=True,
            text=True,
            timeout=PORT_WATCH_TIMEOUT,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    return proc.stdout or ""


def _parse_ss_linux(text: str, port: int) -> list[dict[str, str]]:
    """Parser de `ss -ltnp` para un puerto dado.

    Formado típico:
        State   Recv-Q  Send-Q  Local Address:Port  Peer ...
        LISTEN  0       128     127.0.0.1:1455      0.0.0.0:*
        LISTEN  0       128     *:54545             0.0.0.0:*
    """
    results: list[dict[str, str]] = []
    port_s = f":{port}"
    for line in text.splitlines():
        if "LISTEN" not in line:
            continue
        # Columna de Local Address (4ta en adelante).
        parts = line.split()
        if len(parts) < 4:
            continue
        local = parts[3]
        if not local.endswith(port_s) and f":{port} " not in line + " ":
            continue
        # Asegurar match exacto de puerto (no substring tipo 14550).
        if not _addr_has_port(local, port):
            continue
        pid = ""
        if "users:" in line:
            pid = line.split("users:(", 1)[-1].split(")", 1)[0]
        results.append({"proto": "tcp", "addr": local, "port": str(port), "pid": pid})
    return results


def _parse_netstat_windows(text: str, port: int) -> list[dict[str, str]]:
    """Parser de `netstat -ano` (salida en-US típica) para un puerto.

    Formato:
        Proto  Local Address          Foreign Address        State           PID
        TCP    127.0.0.1:1455         0.0.0.0:0              LISTENING       1234
        TCP6   [::1]:5112             [::]:0                 LISTENING       5678
    """
    results: list[dict[str, str]] = []
    for line in text.splitlines():
        parts = line.split()
        if len(parts) < 5:
            continue
        if parts[0].upper() not in ("TCP", "TCP6"):
            continue
        state = parts[-2].upper()
        if state != "LISTENING":
            continue
        local = parts[1]
        pid = parts[-1]
        if not _addr_has_port(local, port):
            continue
        results.append(
            {"proto": parts[0].lower(), "addr": local, "port": str(port), "pid": pid}
        )
    return results


def _parse_lsof_macos(text: str, port: int) -> list[dict[str, str]]:
    """Parser de `lsof -iTCP -sTCP:LISTEN -P -n` para un puerto.

    Formato:
        COMMAND   PID  USER  FD  TYPE  DEVICE  SIZE/OFF  NODE NAME
        cli-prox  1234 user   8u  IPv4  0x...   0t0       TCP 127.0.0.1:1455 (LISTEN)
    """
    results: list[dict[str, str]] = []
    for line in text.splitlines():
        if "LISTEN" not in line:
            continue
        parts = line.split()
        if len(parts) < 9:
            continue
        name_idx = 8
        name = parts[name_idx] if len(parts) > name_idx else ""
        if not _addr_has_port(name, port):
            continue
        pid = parts[1] if len(parts) > 1 else ""
        command = parts[0] if parts else ""
        results.append(
            {
                "proto": "tcp",
                "addr": name,
                "port": str(port),
                "pid": pid,
                "command": command,
            }
        )
    return results


def _addr_has_port(addr: str, port: int) -> bool:
    """True si addr termina en :<port> exacto (evita 14550 == 1455)."""
    port_s = str(port)
    # Caso IPv6 [::1]:port
    if "]:" in addr:
        after = addr.rsplit("]:", 1)[-1]
        return after == port_s
    if ":" not in addr:
        return False
    return addr.rsplit(":", 1)[-1] == port_s


# ─── Orquestador + CLI ────────────────────────────────────────────────────────


def discover(auth_url: str) -> dict[str, Any]:
    """Descubre callback + diagnostica listener. Devuelve dict JSON-safe."""
    cb = parse_callback(auth_url)
    listener = diagnose_listener(cb.host, cb.port)
    return {
        "ok": True,
        "callback": asdict(cb),
        "listener": asdict(listener),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Descubre el callback OAuth esperado desde la auth_url y diagnostica "
            "si el listener local está activo. No emite code/token/state."
        )
    )
    parser.add_argument(
        "--url",
        help="Auth URL a parsear (si se omite, se lee JSON de stdin).",
    )
    parser.add_argument(
        "--diagnose",
        action="store_true",
        help="Solo diagnosticar listener para --host/--port dados.",
    )
    parser.add_argument("--host", default=None)
    parser.add_argument("--port", type=int, default=None)
    args = parser.parse_args(argv)

    if args.diagnose:
        if not args.host or not args.port:
            print(
                json.dumps(
                    {"ok": False, "message": "--diagnose requiere --host y --port."}
                )
            )
            return 1
        listener = diagnose_listener(args.host, args.port)
        print(json.dumps({"ok": True, "listener": asdict(listener)}))
        return 0

    # Modo default: parsear auth_url (+ diagnosticar).
    if args.url:
        auth_url = args.url
    else:
        try:
            req = json.loads(sys.stdin.read())
        except (ValueError, OSError):
            print(json.dumps({"ok": False, "message": "stdin JSON inválido."}))
            return 1
        auth_url = str(req.get("url", "") or "")

    result = discover(auth_url)
    # CRÍTICO: stdout SOLO lleva callback (host/port/path) + listener.
    # Nunca code/token/state (no están en la auth_url de todos modos, pero
    # por defensa el parser los descarta explícitamente).
    print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
