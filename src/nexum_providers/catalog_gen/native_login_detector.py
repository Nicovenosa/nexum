"""Nexum Native Login Detector — safe detection of CLI OAuth credentials (ADR-044 Capa 1).

Detects whether a native CLI login exists on disk WITHOUT reading token values.
Only records: existence, path, mtime, size, and a "probable type" hint. If a JSON
file is parsed to determine type, sensitive fields (token/access/refresh/key/secret)
are explicitly IGNORED — their values are never read, stored, or printed.

This module never: reads token values, copies tokens, prints secrets, or makes
network calls. It is the safe input layer for the CLIProxyAPI bridge activation.

Stdlib only.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .provider_registry import ProviderDefinition


# JSON keys that hold secret values — NEVER read/store these.
SENSITIVE_JSON_KEYS = frozenset(
    {
        "token",
        "access",
        "refresh",
        "key",
        "secret",
        "accessToken",
        "refreshToken",
        "access_token",
        "refresh_token",
        "apiKey",
        "api_key",
        "password",
        "credential",
        "claudeAiOauth",  # Claude Code nested OAuth object
    }
)


@dataclass
class NativeLoginInfo:
    """Safe record of a detected native login. No secret values."""

    provider_id: str
    detected: bool
    credential_path: str | None = None
    exists: bool = False
    mtime: float | None = None
    size: int | None = None
    probable_type: str | None = None  # "oauth" | "api" | "config" | "dir" | "unknown"
    # Safe structural hints (e.g. JSON top-level keys, minus sensitive ones).
    safe_keys: list[str] | None = None
    error: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "provider_id": self.provider_id,
            "detected": self.detected,
            "credential_path": self.credential_path,
            "exists": self.exists,
            "mtime": self.mtime,
            "size": self.size,
            "probable_type": self.probable_type,
            "safe_keys": self.safe_keys,
            "error": self.error,
        }


def _expand(path: str) -> Path:
    return Path(os.path.expanduser(path))


def _first_existing(candidates: tuple[str, ...]) -> Path | None:
    for raw in candidates:
        p = _expand(raw)
        if p.exists():
            return p
    return None


def _safe_json_keys(path: Path) -> tuple[list[str], str | None]:
    """Parse JSON to extract NON-sensitive top-level keys + probable type.

    Returns (safe_keys, probable_type). NEVER returns secret values. If a key
    is in SENSITIVE_JSON_KEYS, it is included in safe_keys only as a NAME (to
    indicate presence) but its value is never read.
    """
    try:
        data = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except (OSError, ValueError):
        return [], None

    if not isinstance(data, dict):
        return [], "non_object_json"

    # Record top-level key NAMES only (presence indicator), never values.
    keys = list(data.keys())

    # Determine probable type from safe structural hints.
    probable_type = "unknown"
    # Check for a nested type field without reading secrets.
    for k, v in data.items():
        if k in SENSITIVE_JSON_KEYS:
            continue
        if k == "type" and isinstance(v, str):
            probable_type = v  # "api" | "oauth"
            break
    # If any sensitive key is present, infer oauth/api.
    if probable_type == "unknown":
        has_access = any(k in data for k in ("access", "accessToken", "access_token"))
        has_refresh = any(
            k in data for k in ("refresh", "refreshToken", "refresh_token")
        )
        has_key = any(k in data for k in ("key", "apiKey", "api_key"))
        if has_access or has_refresh:
            probable_type = "oauth"
        elif has_key:
            probable_type = "api"

    return keys, probable_type


def detect_native_login(definition: ProviderDefinition) -> NativeLoginInfo:
    """Detect a native login for a provider. Safe: never reads secrets.

    For directory paths (trailing slash or non-file), records existence + lists
    safe child names (no contents). For files, records mtime/size + safe JSON
    structure (keys only, values of sensitive keys ignored).
    """
    pid = definition.provider_id
    candidates = definition.native_credential_paths
    if not candidates:
        return NativeLoginInfo(provider_id=pid, detected=False)

    path = _first_existing(candidates)
    if path is None:
        return NativeLoginInfo(
            provider_id=pid,
            detected=False,
            error="no credential path found",
        )

    # Directory case (e.g. MiMo data-dir, OpenCode config dir).
    if path.is_dir():
        try:
            children = sorted(p.name for p in path.iterdir())
        except OSError as exc:
            return NativeLoginInfo(
                provider_id=pid,
                detected=True,
                credential_path=str(path),
                exists=True,
                probable_type="dir",
                error=f"cannot list dir: {exc}",
            )
        return NativeLoginInfo(
            provider_id=pid,
            detected=True,
            credential_path=str(path),
            exists=True,
            probable_type="dir",
            safe_keys=children,
        )

    # File case.
    try:
        stat = path.stat()
    except OSError as exc:
        return NativeLoginInfo(
            provider_id=pid,
            detected=True,
            credential_path=str(path),
            exists=True,
            error=f"cannot stat: {exc}",
        )

    info = NativeLoginInfo(
        provider_id=pid,
        detected=True,
        credential_path=str(path),
        exists=True,
        mtime=stat.st_mtime,
        size=stat.st_size,
    )

    # Try to extract safe structural hints from JSON files.
    if path.suffix in (".json",) or path.name.endswith(".json"):
        safe_keys, probable_type = _safe_json_keys(path)
        info.safe_keys = safe_keys
        info.probable_type = probable_type
    else:
        info.probable_type = "config"

    return info


def detect_all_native(
    registry: dict[str, ProviderDefinition],
) -> dict[str, NativeLoginInfo]:
    """Detect native logins for all providers with native_credential_paths."""
    out: dict[str, NativeLoginInfo] = {}
    for pid, definition in registry.items():
        if not definition.native_credential_paths:
            continue
        out[pid] = detect_native_login(definition)
    return out
