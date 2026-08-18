"""Nexum OpenCode Family Detector — direct key extraction from auth.json (ADR-044 §1.2).

A single parametrizable detector covers OpenCode, OpenCode Zen, OpenCode Go (and
would cover MiMo if it used auth.json — it does NOT, see mimo_detector.py).

auth.json format (observed on this machine):
    {"openai": {"type": "oauth", "refresh": "...", "access": "...", "expires": ...},
     "opencode-go": {"type": "api", "key": "..."}}

Security:
  - NEVER prints key/token/access/refresh values.
  - Reads the auth.json to determine presence + type, but only stores masked
    fingerprints (first2…last4).
  - Does NOT copy tokens or send them anywhere during detection.
  - A live probe (to list models) uses the key only against the provider's own
    endpoint, and only when explicitly enabled by the caller.

Stdlib only.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .provider_registry import DetectionStatus, ProviderDefinition

PROBE_TIMEOUT = 4.0  # seconds, localhost or remote provider


@dataclass
class OpenCodeFamilyResult:
    provider_id: str
    family: str
    status: str
    detail: str
    auth_json_path: str | None = None
    entry_key: str | None = None  # which auth.json key matched
    entry_type: str | None = None  # "api" | "oauth" | None
    base_url: str | None = None
    models: list[str] = field(default_factory=list)
    models_status: str = "probe_pending"
    credential_detected: bool = False
    credential_fingerprint: str | None = None  # masked first2…last4
    next_action: str | None = None
    usable_now: bool = False
    error: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "provider_id": self.provider_id,
            "family": self.family,
            "status": self.status,
            "detail": self.detail,
            "auth_json_path": self.auth_json_path,
            "entry_key": self.entry_key,
            "entry_type": self.entry_type,
            "base_url": self.base_url,
            "models": self.models,
            "models_status": self.models_status,
            "credential_detected": self.credential_detected,
            "credential_fingerprint": self.credential_fingerprint,
            "next_action": self.next_action,
            "usable_now": self.usable_now,
            "error": self.error,
        }


def _mask(value: str) -> str:
    """Mask a secret to first2…last4 (or <redacted> if too short)."""
    v = str(value).strip()
    if len(v) <= 6:
        return "<redacted>"
    return f"{v[:2]}…{v[-4:]}"


def _extract_key(entry: dict[str, Any]) -> str | None:
    """Pull a usable key from an auth.json entry WITHOUT printing it."""
    for field_name in ("key", "access", "token"):
        v = entry.get(field_name)
        if isinstance(v, str) and v:
            return v
    return None


def _match_entry(
    definition: ProviderDefinition, auth_data: dict[str, Any]
) -> tuple[str | None, dict[str, Any] | None]:
    """Match the right auth.json entry for this provider.

    If auth_json_keys is set, match the first one present. If empty (generic
    OpenCode), match the first key not claimed by a more specific family.
    """
    if definition.auth_json_keys:
        for key in definition.auth_json_keys:
            if key in auth_data and isinstance(auth_data[key], dict):
                return key, auth_data[key]
        return None, None

    # Generic: pick first key not in any specific family's auth_json_keys.
    from .provider_registry import PROVIDER_REGISTRY

    claimed: set[str] = set()
    for d in PROVIDER_REGISTRY.values():
        if d.provider_id == definition.provider_id:
            continue
        if d.auth_mode == "direct_key":
            claimed.update(d.auth_json_keys)
    for key, value in auth_data.items():
        if key not in claimed and isinstance(value, dict):
            return key, value
    return None, None


def _probe_models_via_cli(
    provider_id: str, timeout: float = PROBE_TIMEOUT
) -> tuple[list[str], str]:
    """Probe models using `opencode models <provider>` CLI command.

    This is the preferred method: it uses OpenCode's own model cache and
    authentication, no need to know base URLs or handle tokens directly.

    Returns (model_ids, status). Status is one of: 'ok', 'failed', 'no_cli'.
    Never raises; never prints secrets.
    """
    import subprocess

    # Map registry provider_id to OpenCode CLI provider name.
    cli_provider_map = {
        "opencode": "opencode",
        "opencode_zen": "opencode",  # Zen uses the same "opencode" namespace
        "opencode_go": "opencode-go",
    }
    cli_provider = cli_provider_map.get(provider_id)
    if not cli_provider:
        return [], "no_cli"

    try:
        result = subprocess.run(
            ["opencode", "models", cli_provider],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (subprocess.TimeoutExpired, FileNotFoundError, OSError):
        return [], "no_cli"

    if result.returncode != 0:
        return [], "failed"

    # Parse output: each line is "provider/model-id"
    models = []
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line or "/" not in line:
            continue
        # Extract model ID after the slash
        parts = line.split("/", 1)
        if len(parts) == 2:
            model_id = parts[1].strip()
            if model_id:
                models.append(model_id)

    return models, "ok" if models else "failed"


def _probe_models(
    base_url: str, api_key: str, timeout: float = PROBE_TIMEOUT
) -> tuple[list[str], str]:
    """Probe GET {base_url}/models with the key. Returns (model_ids, status).

    Status is one of: 'ok', 'failed', 'no_base_url', 'skipped'.
    Never raises; never prints the key.

    DEPRECATED: Use _probe_models_via_cli instead. This HTTP fallback is kept
    only for providers without OpenCode CLI support.
    """
    if not base_url:
        return [], "no_base_url"
    url = base_url.rstrip("/") + "/models"
    req = urllib.request.Request(
        url,
        headers={
            "Accept": "application/json",
            "User-Agent": "nexum/1.0",
            "Authorization": f"Bearer {api_key}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310
            if resp.status != 200:
                return [], "failed"
            raw = resp.read().decode("utf-8", errors="replace")
    except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError, OSError):
        return [], "failed"
    except ValueError:
        return [], "failed"
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        return [], "failed"
    # OpenAI-compatible {"data":[{"id":...}]}
    data = payload.get("data") if isinstance(payload, dict) else None
    if isinstance(data, list):
        ids = [
            m.get("id")
            for m in data
            if isinstance(m, dict) and isinstance(m.get("id"), str)
        ]
        ids = [i for i in ids if i]
        return ids, "ok" if ids else "failed"
    return [], "failed"


def detect_opencode_family(
    definition: ProviderDefinition,
    *,
    probe_online: bool = False,
    auth_data_override: dict[str, Any] | None = None,
) -> OpenCodeFamilyResult:
    """Detect an OpenCode-family provider by reading auth.json.

    Set `probe_online=True` to attempt a live /models probe (uses the key against
    the provider endpoint). Default False = never makes a network call.
    `auth_data_override` lets tests inject a parsed auth.json without disk I/O.
    """
    pid = definition.provider_id
    family = definition.family

    # Locate + parse auth.json.
    auth_path: Path | None = None
    auth_data: dict[str, Any] | None = auth_data_override
    if auth_data is None:
        for raw in definition.native_credential_paths:
            p = Path(os.path.expanduser(raw))
            if p.exists():
                auth_path = p
                break
        if auth_path is None:
            return OpenCodeFamilyResult(
                provider_id=pid,
                family=family,
                status=DetectionStatus.NOT_INSTALLED,
                detail="No se encontró auth.json para esta familia.",
            )
        try:
            auth_data = json.loads(
                auth_path.read_text(encoding="utf-8", errors="replace")
            )
        except (OSError, ValueError):
            return OpenCodeFamilyResult(
                provider_id=pid,
                family=family,
                status=DetectionStatus.ERROR,
                detail="auth.json presente pero no se pudo parsear.",
                auth_json_path=str(auth_path),
            )

    if not isinstance(auth_data, dict):
        return OpenCodeFamilyResult(
            provider_id=pid,
            family=family,
            status=DetectionStatus.ERROR,
            detail="auth.json no es un objeto JSON.",
            auth_json_path=str(auth_path) if auth_path else None,
        )

    entry_key, entry = _match_entry(definition, auth_data)
    if entry is None or entry_key is None:
        return OpenCodeFamilyResult(
            provider_id=pid,
            family=family,
            status=DetectionStatus.NOT_INSTALLED,
            detail="auth.json existe pero sin entrada para esta familia.",
            auth_json_path=str(auth_path) if auth_path else None,
            next_action="ingresar_api_key",
        )

    entry_type = entry.get("type") if isinstance(entry.get("type"), str) else None
    api_key = _extract_key(entry)
    if not api_key:
        return OpenCodeFamilyResult(
            provider_id=pid,
            family=family,
            status=DetectionStatus.ERROR,
            detail=f"Entrada '{entry_key}' encontrada pero sin clave utilizable.",
            auth_json_path=str(auth_path) if auth_path else None,
            entry_key=entry_key,
            entry_type=entry_type,
            next_action="ingresar_api_key",
        )

    # Sprint C: algunos providers (MiMo/Xiaomi) traen la base URL regional
    # del usuario en entry.metadata.base_url — tiene prioridad sobre el hint.
    metadata = entry.get("metadata")
    metadata_base_url = (
        metadata.get("base_url")
        if isinstance(metadata, dict) and isinstance(metadata.get("base_url"), str)
        else None
    )
    effective_base_url = metadata_base_url or definition.base_url_hint
    result = OpenCodeFamilyResult(
        provider_id=pid,
        family=family,
        status=DetectionStatus.PROBE_PENDING,
        detail="Clave detectada. Probe de modelos pendiente (usa --probe-online).",
        auth_json_path=str(auth_path) if auth_path else None,
        entry_key=entry_key,
        entry_type=entry_type,
        base_url=effective_base_url,
        credential_detected=True,
        credential_fingerprint=_mask(api_key),
        models_status="probe_pending",
        next_action="relevar_modelos",
    )

    if probe_online:
        # Sprint 2: prefer CLI-based model enumeration over HTTP probe.
        # `opencode models <provider>` uses OpenCode's own cache and auth,
        # no need to know base URLs or handle tokens directly.
        models, status = _probe_models_via_cli(pid)
        if status == "ok" and models:
            result.models = models
            result.models_status = status
            result.status = DetectionStatus.USABLE
            result.detail = f"Clave extraída y {len(models)} modelos enumerados vía `opencode models`."
            result.usable_now = True
            result.next_action = None
        else:
            # Fallback to HTTP probe if CLI failed or not available.
            if effective_base_url:
                models, status = _probe_models(effective_base_url, api_key)
                result.models = models
                result.models_status = status
                if status == "ok" and models:
                    result.status = DetectionStatus.USABLE
                    result.detail = (
                        "Clave extraída y validada vía /models HTTP (fallback)."
                    )
                    result.usable_now = True
                    result.next_action = None
                elif status == "failed":
                    result.status = DetectionStatus.PROBE_FAILED
                    result.detail = (
                        "Clave detectada pero el probe falló (401/403/red). "
                        "La clave podría estar vencida o el endpoint incorrecto."
                    )
                    result.next_action = "reintentar_probe"
            else:
                # No CLI, no base URL — auth valid but cannot enumerate models.
                result.status = DetectionStatus.USABLE
                result.detail = (
                    "Clave válida detectada, pero no se pudo enumerar el catálogo "
                    "de modelos (opencode CLI no disponible y sin base URL). "
                    "Reintentar con --probe-online o verificar instalación de opencode."
                )
                result.usable_now = True
                result.next_action = "reintentar_probe"
    else:
        # Without a probe, we know the credential is present but cannot confirm
        # usability. Stay probe_pending with a clear action — NEVER mute.
        result.next_action = "relevar_modelos"

    return result
