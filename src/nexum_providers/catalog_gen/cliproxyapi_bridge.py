"""Nexum CLIProxyAPI Bridge — lifecycle + Management API client (ADR-044 Capa 2).

CLIProxyAPI is a local Go service that bridges native CLI OAuth logins (Claude
Code, Codex, Gemini) into a usable OpenAI-compatible endpoint. This module:

  - detects whether it's installed (`is_installed`)
  - detects whether it's running on 127.0.0.1:8317 (`is_running`)
  - starts it via `systemctl --user start cli-proxy-api` (`ensure_running`)
  - queries the Management API for real auth status (`list_auth_files`)
  - kicks off + polls the OAuth login flow for a provider

Security:
  - NEVER prints the management key or any tokens.
  - The management key is read from CLIPROXYAPI_MANAGEMENT_KEY env once and sent
    only as an Authorization header to localhost.
  - If the management key is required but absent, returns
    `bridge_management_locked` (never crashes, never silently proceeds).
  - All network calls are to 127.0.0.1 only.

Uses urllib (stdlib) — no httpx/requests, matching the project's stdlib-only rule.
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .provider_registry import DetectionStatus

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 8317
DEFAULT_SYSTEMD_UNIT = "cli-proxy-api"
DEFAULT_BINARY_NAME = "cli-proxy-api"
PROBE_TIMEOUT = 0.5  # seconds, localhost only
HTTP_TIMEOUT = 4.0  # seconds, localhost only


# ─── Bridge status ────────────────────────────────────────────────────────────


@dataclass
class BridgeStatus:
    """Overall status of the CLIProxyAPI bridge."""

    installed: bool = False
    running: bool = False
    port: int = DEFAULT_PORT
    binary_path: str | None = None
    config_path: str | None = None
    management_key_present: bool = False
    status: str = DetectionStatus.BRIDGE_NOT_INSTALLED  # high-level status string
    detail: str = ""
    auth_files: list[dict[str, Any]] = field(default_factory=list)
    error: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "installed": self.installed,
            "running": self.running,
            "port": self.port,
            "binary_path": self.binary_path,
            "config_path": self.config_path,
            "management_key_present": self.management_key_present,
            "status": self.status,
            "detail": self.detail,
            "auth_file_count": len(self.auth_files),
            # auth_files items are safe (provider/email/status/last_refresh — no tokens).
            # But we DO NOT include the raw list in to_dict to keep the catalog small;
            # callers extract what they need.
            "error": self.error,
        }


# ─── Bridge client ────────────────────────────────────────────────────────────


class CLIProxyAPIBridge:
    """Client + lifecycle manager for CLIProxyAPI."""

    def __init__(
        self,
        host: str = DEFAULT_HOST,
        port: int = DEFAULT_PORT,
        management_key: str | None = None,
        systemd_unit: str = DEFAULT_SYSTEMD_UNIT,
        binary_name: str = DEFAULT_BINARY_NAME,
    ) -> None:
        self.host = host
        self.port = port
        self.base_url = f"http://{host}:{port}"
        self.mgmt_url = f"{self.base_url}/v0/management"
        self.systemd_unit = systemd_unit
        self.binary_name = binary_name
        self.management_key = management_key or os.environ.get(
            "CLIPROXYAPI_MANAGEMENT_KEY"
        )

    # ── Installation + lifecycle ──

    def is_installed(self) -> bool:
        """True if the binary is on PATH or a known package is installed."""
        if shutil.which(self.binary_name):
            return True
        # Try package managers (AUR/pacman on Arch).
        for pkgmgr, args in (("paru", ("-Qi",)), ("pacman", ("-Qi",))):
            if not shutil.which(pkgmgr):
                continue
            for pkg in (f"{self.binary_name}-bin", self.binary_name):
                try:
                    proc = subprocess.run(
                        [pkgmgr, *args, pkg],
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                        timeout=5,
                    )
                    if proc.returncode == 0:
                        return True
                except (OSError, subprocess.SubprocessError):
                    continue
        return False

    def binary_location(self) -> str | None:
        loc = shutil.which(self.binary_name)
        return loc if loc else None

    def is_running(self, timeout: float = PROBE_TIMEOUT) -> bool:
        """True if a service answers on host:port (TCP probe, localhost only)."""
        try:
            with socket.create_connection((self.host, self.port), timeout=timeout):
                return True
        except OSError:
            return False

    def ensure_running(self, attempts: int = 10, delay: float = 0.3) -> bool:
        """Start the service via systemd --user if installed + not running.

        Returns True if it ended up running. Does NOT install the package.
        """
        if self.is_running():
            return True
        if not self.is_installed():
            return False
        if not shutil.which("systemctl"):
            return False
        try:
            subprocess.run(
                ["systemctl", "--user", "start", self.systemd_unit],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=10,
                check=False,
            )
        except (OSError, subprocess.SubprocessError):
            return False
        # Wait for the socket to open.
        for _ in range(attempts):
            if self.is_running():
                return True
            import time

            time.sleep(delay)
        return False

    # ── Management API ──

    def _headers(self) -> dict[str, str]:
        if not self.management_key:
            return {"Accept": "application/json"}
        return {
            "Accept": "application/json",
            "Authorization": f"Bearer {self.management_key}",
        }

    def _localhost_get(self, url: str) -> dict[str, Any] | list[Any] | None:
        """GET a localhost URL, returning parsed JSON or None on failure.

        Refuses non-localhost URLs (defense in depth).
        """
        if not url.startswith(f"http://{self.host}"):
            return None
        req = urllib.request.Request(url, headers=self._headers())
        try:
            with urllib.request.urlopen(req, timeout=HTTP_TIMEOUT) as resp:  # noqa: S310
                if resp.status != 200:
                    return None
                raw = resp.read().decode("utf-8", errors="replace")
        except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError, OSError):
            return None
        except ValueError:
            return None
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return None

    def list_auth_files(self) -> list[dict[str, Any]]:
        """GET /auth-files — the source of truth for which accounts are bridged.

        Returns a list of dicts with safe fields (provider, status, email,
        last_refresh, ...). Token values are never present in this endpoint's
        response.
        """
        data = self._localhost_get(f"{self.mgmt_url}/auth-files")
        if data is None:
            return []
        if isinstance(data, list):
            return [d for d in data if isinstance(d, dict)]
        if isinstance(data, dict):
            # 7.2.50 real: {"files": [...]}. Versiones/mocks previos usaban
            # "auth_files"/"data". Sin "files" acá, el catálogo clasificaba
            # TODO como bridge_not_active aun con tokens activos (bug
            # post-login E2E 2026-07-06).
            files = (
                data.get("files") or data.get("auth_files") or data.get("data") or []
            )
            return [d for d in files if isinstance(d, dict)]
        return []

    def start_oauth_login(self, cliproxy_provider_id: str) -> tuple[str, str] | None:
        """GET /{provider}-auth-url → (auth_url, state) or None on failure.

        The caller is responsible for opening auth_url and polling
        poll_oauth_status. Never reads native tokens.
        """
        data = self._localhost_get(f"{self.mgmt_url}/{cliproxy_provider_id}-auth-url")
        if not isinstance(data, dict):
            return None
        url = data.get("url")
        state = data.get("state")
        if isinstance(url, str) and isinstance(state, str):
            return url, state
        return None

    def poll_oauth_status(self, state: str) -> str:
        """GET /get-auth-status?state=... → 'wait' | 'ok' | 'error'."""
        data = self._localhost_get(f"{self.mgmt_url}/get-auth-status?state={state}")
        if isinstance(data, dict):
            s = data.get("status")
            if isinstance(s, str):
                return s
        return "error"

    def list_models(self) -> list[dict[str, Any]]:
        """GET /v1/models (inference API) → [{"id", "owned_by"}, ...].

        Uses CLIPROXYAPI_API_KEY from env if present (the inference API keys
        configured in config.yaml). Returns [] on any failure. Localhost only.
        """
        api_key = os.environ.get("CLIPROXYAPI_API_KEY")
        url = f"{self.base_url}/v1/models"
        if not url.startswith(f"http://{self.host}"):
            return []
        headers = {"Accept": "application/json"}
        if api_key:
            headers["Authorization"] = f"Bearer {api_key}"
        req = urllib.request.Request(url, headers=headers)
        try:
            with urllib.request.urlopen(req, timeout=HTTP_TIMEOUT) as resp:  # noqa: S310
                if resp.status != 200:
                    return []
                raw = resp.read().decode("utf-8", errors="replace")
        except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError, OSError):
            return []
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            return []
        data = payload.get("data") if isinstance(payload, dict) else None
        if not isinstance(data, list):
            return []
        out: list[dict[str, Any]] = []
        for m in data:
            if isinstance(m, dict) and isinstance(m.get("id"), str):
                out.append({"id": m["id"], "owned_by": m.get("owned_by")})
        return out

    # ── High-level status ──

    def status(self) -> BridgeStatus:
        """Compute the overall bridge status (installed/running/management/auth)."""
        installed = self.is_installed()
        running = self.is_running()
        mgmt_present = bool(self.management_key)
        binary = self.binary_location()
        config = str(Path.home() / ".cli-proxy-api" / "config.yaml")

        st = BridgeStatus(
            installed=installed,
            running=running,
            port=self.port,
            binary_path=binary,
            config_path=config if Path(config).exists() else None,
            management_key_present=mgmt_present,
        )

        if not installed:
            st.status = DetectionStatus.BRIDGE_NOT_INSTALLED
            st.detail = (
                "CLIProxyAPI no está instalado. Instalalo manualmente: "
                "paru -S cli-proxy-api-bin"
            )
            return st

        if not running:
            st.status = DetectionStatus.BRIDGE_NOT_RUNNING
            st.detail = (
                "CLIProxyAPI instalado pero no corriendo. Iniciarlo: "
                "systemctl --user start cli-proxy-api"
            )
            return st

        # Running — try to fetch auth files.
        auth_files = self.list_auth_files()
        st.auth_files = auth_files
        if not auth_files and not mgmt_present:
            # Likely needs management key to see auth files.
            st.status = DetectionStatus.BRIDGE_MANAGEMENT_LOCKED
            st.detail = (
                "CLIProxyAPI corre pero la Management API requiere "
                "CLIPROXYAPI_MANAGEMENT_KEY para listar cuentas activas."
            )
            return st

        st.status = "bridge_ok"
        st.detail = (
            f"CLIProxyAPI activo en :{self.port}, "
            f"{len(auth_files)} cuenta(s) puenteada(s)."
        )
        return st


# Provider-name aliases: the auth-url endpoint prefix and the `provider` field
# reported by GET /auth-files do not always coincide across CLIProxyAPI
# versions (e.g. anthropic-auth-url → provider "claude"). Match tolerantly.
_PROVIDER_ALIASES: dict[str, tuple[str, ...]] = {
    "anthropic": ("anthropic", "claude"),
    "codex": ("codex", "openai"),
    "antigravity": ("antigravity", "gemini", "gemini-cli"),
}


def auth_file_for(
    auth_files: list[dict[str, Any]], cliproxy_provider_id: str
) -> dict[str, Any] | None:
    """Find the auth-file entry matching a CLIProxyAPI provider id."""
    accepted = _PROVIDER_ALIASES.get(cliproxy_provider_id, (cliproxy_provider_id,))
    for entry in auth_files:
        if str(entry.get("provider", "")).lower() in accepted:
            return entry
    return None
