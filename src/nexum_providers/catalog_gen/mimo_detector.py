"""Nexum MiMo Code Detector — detects MiMo's DIFFERENT auth format (ADR-044 §1.2 caveat).

MiMo Code (Xiaomi) shares OpenCode lineage but does NOT use auth.json on this
machine. It uses:
  - ~/.local/share/mimocode/mimocode.db  (SQLite — not safe to parse for keys)
  - ~/.local/share/mimocode/mimo-key-name (references a keyring-backed key)
  - ~/.config/mimocode/mimocode.json     (config with provider/model fields)

This detector confirms MiMo's presence via these paths WITHOUT touching the
SQLite DB or keyring (out of safe scope). It reports `mimo_detected_different_format`
so /proveedor can show MiMo exists with a clear "different auth format" note +
next_action, rather than hiding it or pretending it's usable.

Stdlib only. No token reads.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .provider_registry import DetectionStatus, ProviderDefinition


MIMO_DATA_DIRS = (
    "~/.local/share/mimocode/",
    "~/.local/share/mimo/",
    "~/.local/share/mimo-code/",
)
MIMO_CONFIG_PATHS = (
    "~/.config/mimocode/mimocode.json",
    "~/.config/mimo/mimo.json",
)
MIMO_KEYNAME_FILE = "mimo-key-name"


@dataclass
class MiMoResult:
    provider_id: str
    family: str
    status: str
    detail: str
    data_dir: str | None = None
    config_path: str | None = None
    detected_files: list[str] = field(default_factory=list)
    config_provider: str | None = None  # safe field from mimocode.json
    config_model: str | None = None  # safe field from mimocode.json
    next_action: str | None = None
    usable_now: bool = False
    error: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "provider_id": self.provider_id,
            "family": self.family,
            "status": self.status,
            "detail": self.detail,
            "data_dir": self.data_dir,
            "config_path": self.config_path,
            "detected_files": self.detected_files,
            "config_provider": self.config_provider,
            "config_model": self.config_model,
            "next_action": self.next_action,
            "usable_now": self.usable_now,
            "error": self.error,
        }


def _safe_read_mimo_config(path: Path) -> tuple[str | None, str | None]:
    """Read mimocode.json for the SAFE 'provider' and 'model' fields only.

    Never reads/returns key or token fields. Returns (provider, model).
    """
    try:
        data = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except (OSError, ValueError):
        return None, None
    if not isinstance(data, dict):
        return None, None
    provider = data.get("provider")
    model = data.get("model")
    return (
        provider if isinstance(provider, str) else None,
        model if isinstance(model, str) else None,
    )


def detect_mimo(definition: ProviderDefinition) -> MiMoResult:
    """Detect MiMo Code by its non-auth.json footprint.

    Confirms data-dir + config presence. Does NOT parse the SQLite DB (would
    risk exposing key material). Reports `mimo_detected_different_format`.
    """
    pid = definition.provider_id
    family = definition.family

    # Locate data dir.
    data_dir: Path | None = None
    detected_files: list[str] = []
    for raw in MIMO_DATA_DIRS:
        p = Path(os.path.expanduser(raw))
        if p.is_dir():
            data_dir = p
            try:
                detected_files = sorted(child.name for child in p.iterdir())
            except OSError:
                detected_files = []
            break

    # Locate config.
    config_path: Path | None = None
    config_provider = None
    config_model = None
    for raw in MIMO_CONFIG_PATHS:
        p = Path(os.path.expanduser(raw))
        if p.exists():
            config_path = p
            config_provider, config_model = _safe_read_mimo_config(p)
            break

    if data_dir is None and config_path is None:
        return MiMoResult(
            provider_id=pid,
            family=family,
            status=DetectionStatus.NOT_INSTALLED,
            detail="No se encontró data-dir ni config de MiMo Code.",
        )

    # MiMo IS detected, but its auth format is incompatible with the OpenCode
    # family detector (SQLite + keyring). We cannot extract a usable key safely.
    parts = []
    if data_dir:
        parts.append(f"data-dir {data_dir}")
    if config_path:
        parts.append(f"config {config_path}")

    return MiMoResult(
        provider_id=pid,
        family=family,
        status="mimo_detected_different_format",
        detail=(
            "MiMo Code detectado pero usa un formato de auth diferente "
            "(SQLite + keyring, no auth.json). No se puede extraer clave "
            "de forma segura en V0. " + (" | ".join(parts))
        ),
        data_dir=str(data_dir) if data_dir else None,
        config_path=str(config_path) if config_path else None,
        detected_files=detected_files,
        config_provider=config_provider,
        config_model=config_model,
        next_action="mimo_adapter_required",
        usable_now=False,
    )
