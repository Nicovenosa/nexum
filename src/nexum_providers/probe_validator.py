"""Nexum Probe Validator — validate API keys via live probe (Sprint 4).

Reuses the probe infrastructure pattern from opencode_family_detector.py.
Supports both Anthropic and OpenAI protocols.

Security:
  - Keys are NEVER logged or printed
  - Timeout: 5s
  - Distinguishes 401/403 (bad key) from network errors (retry)
  - Stdlib only
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from dataclasses import dataclass

PROBE_TIMEOUT = 5.0


@dataclass
class ProbeResult:
    """Result of an API key validation probe."""

    success: bool
    models: list[str]
    error_code: int | None = None
    error_detail: str | None = None
    no_models: bool = False

    @property
    def status_label(self) -> str:
        if self.success:
            return f"usable ({len(self.models)} models)"
        # Auth OK (HTTP 200) but the endpoint returned no models — distinct
        # from a network failure. Usually means the key works but the plan
        # has no model access, so the provider is NOT usable as-is.
        if self.no_models:
            return "auth OK pero el endpoint no listó modelos"
        if self.error_code in (401, 403):
            return "key inválida o sin permisos"
        if self.error_code is not None:
            return f"error HTTP {self.error_code}"
        return "no se pudo contactar al proveedor, reintentar"


def probe_api_key(
    models_endpoint: str,
    api_key: str,
    protocol: str = "openai",
    timeout: float = PROBE_TIMEOUT,
) -> ProbeResult:
    """Validate an API key by probing the models endpoint.

    Args:
        models_endpoint: Full URL to the models endpoint
        api_key: The API key to validate (never logged)
        protocol: "openai" or "anthropic"
        timeout: Request timeout in seconds

    Returns:
        ProbeResult with success status, models list, and error details
    """
    if protocol == "anthropic":
        return _probe_anthropic(models_endpoint, api_key, timeout)
    return _probe_openai(models_endpoint, api_key, timeout)


def _probe_openai(endpoint: str, api_key: str, timeout: float) -> ProbeResult:
    """Probe an OpenAI-compatible /models endpoint."""
    req = urllib.request.Request(
        endpoint,
        headers={
            "Accept": "application/json",
            "User-Agent": "nexum/1.0",
            "Authorization": f"Bearer {api_key}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if resp.status != 200:
                return ProbeResult(
                    success=False,
                    models=[],
                    error_code=resp.status,
                )
            raw = resp.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        return ProbeResult(
            success=False,
            models=[],
            error_code=exc.code,
            error_detail=str(exc.reason),
        )
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        return ProbeResult(
            success=False,
            models=[],
            error_detail=f"network error: {exc}",
        )

    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        return ProbeResult(
            success=False,
            models=[],
            error_detail="invalid JSON response",
        )

    data = payload.get("data") if isinstance(payload, dict) else None
    if isinstance(data, list):
        ids = [
            m.get("id")
            for m in data
            if isinstance(m, dict) and isinstance(m.get("id"), str)
        ]
        ids = [i for i in ids if i]
        if ids:
            return ProbeResult(success=True, models=ids)

    return ProbeResult(
        success=False,
        models=[],
        error_detail="no models found in response",
        no_models=True,
    )


def _probe_anthropic(endpoint: str, api_key: str, timeout: float) -> ProbeResult:
    """Probe Anthropic /models endpoint."""
    req = urllib.request.Request(
        endpoint,
        headers={
            "Accept": "application/json",
            "User-Agent": "nexum/1.0",
            "x-api-key": api_key,
            "anthropic-version": "2023-06-01",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if resp.status != 200:
                return ProbeResult(
                    success=False,
                    models=[],
                    error_code=resp.status,
                )
            raw = resp.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        return ProbeResult(
            success=False,
            models=[],
            error_code=exc.code,
            error_detail=str(exc.reason),
        )
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        return ProbeResult(
            success=False,
            models=[],
            error_detail=f"network error: {exc}",
        )

    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        return ProbeResult(
            success=False,
            models=[],
            error_detail="invalid JSON response",
        )

    data = payload.get("data") if isinstance(payload, dict) else None
    if isinstance(data, list):
        ids = [
            m.get("id")
            for m in data
            if isinstance(m, dict) and isinstance(m.get("id"), str)
        ]
        ids = [i for i in ids if i]
        if ids:
            return ProbeResult(success=True, models=ids)

    return ProbeResult(
        success=False,
        models=[],
        error_detail="no models found in response",
        no_models=True,
    )
