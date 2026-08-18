"""Shared loader for the Nexum reserved-model policy.

Single source of truth: the bundled ``reserved-models.json`` beside this module.
Both the Provider Resolver and the Provider Catalog import this so they agree
on which models are internal_only (reserved for the Hormiguero) and therefore
must NOT be certified as the user-facing runtime model.

The Rust /modelo filter (``peri-tui/src/app/model_panel.rs``) mirrors the same
baseline list via ``DEFAULT_RESERVED_INTERNAL_MODELS``; keep them in sync when
editing this file or that constant.

Security: read-only, no network, no secrets. Pure JSON config load.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

# Default location, relative to this file.
_DEFAULT_PATH = Path(__file__).resolve().parent / "reserved-models.json"

# Hardcoded baseline mirror of the JSON's reserved_models[].model set. Used as a
# defense-in-depth fallback if the JSON is missing/corrupt, AND so callers can
# validate the JSON matches the expected baseline. Must match the Rust const
# DEFAULT_RESERVED_INTERNAL_MODELS in peri-tui/src/app/model_panel.rs.
BASELINE_RESERVED_MODELS: tuple[str, ...] = ("qwen3:0.6b",)


class ReservedPolicyError(RuntimeError):
    """Raised when the reserved-model policy cannot be enforced."""


def load_reserved_policy(path: Path | None = None) -> dict[str, Any]:
    """Load the canonical reserved-models.json.

    Returns the parsed dict. Raises ``ReservedPolicyError`` if missing/invalid.
    """
    p = path or _DEFAULT_PATH
    if not p.is_file():
        raise ReservedPolicyError(f"reserved-models.json not found: {p}")
    try:
        data = json.loads(p.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ReservedPolicyError(f"invalid reserved-models.json: {exc}") from exc
    if not isinstance(data, dict) or not isinstance(data.get("reserved_models"), list):
        raise ReservedPolicyError("reserved-models.json: missing reserved_models list")
    return data


def reserved_model_names(path: Path | None = None) -> set[str]:
    """Return the set of reserved model ids (the user_selectable=false set).

    Falls back to the hardcoded baseline only if the JSON is unreadable, and
    NEVER returns an empty set silently if the baseline is non-empty (that would
    accidentally expose a reserved model). On JSON error, raises.
    """
    data = load_reserved_policy(path)
    names: set[str] = set()
    for entry in data["reserved_models"]:
        if not isinstance(entry, dict):
            continue
        # A model is reserved if explicitly user_selectable=false OR
        # visibility=internal_only OR reserved_for is set. We treat any of these
        # as "reserved" (conservative).
        selectable = entry.get("user_selectable")
        visibility = str(entry.get("visibility", "")).lower()
        reserved_for = entry.get("reserved_for")
        model = entry.get("model")
        if not isinstance(model, str) or not model:
            continue
        if selectable is False or visibility == "internal_only" or reserved_for:
            names.add(model)
    return names


def reserved_entries(path: Path | None = None) -> list[dict[str, str]]:
    """Return the full reserved-model entries (model, reserved_for, reason, ...)."""
    data = load_reserved_policy(path)
    out: list[dict[str, str]] = []
    for entry in data["reserved_models"]:
        if isinstance(entry, dict) and isinstance(entry.get("model"), str):
            out.append(entry)
    return out


def is_reserved(model: str, path: Path | None = None) -> bool:
    """True if ``model`` is in the reserved set (case-sensitive, exact match)."""
    return model in reserved_model_names(path)
