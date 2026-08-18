"""Nexum Key Store — secure storage for user-entered API keys (Sprint 4).

Keys are stored in a JSON file with 600 permissions in the Nexum data directory.
Keys are NEVER logged, printed, or exposed in catalog output. Only masked
fingerprints (first2…last4) are shown to the user.

Security:
  - File permissions: 0o600 (owner read/write only)
  - Keys masked in all output: sk-…x4Kp
  - No Nexum runtime imports
  - Stdlib only
"""

from __future__ import annotations

import json
import os
import stat
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


def _default_store_path() -> Path:
    """Default path for the key store file."""
    data_home = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share")
    data_dir = data_home / "nexum/providers"
    data_dir.mkdir(parents=True, exist_ok=True)
    return data_dir / "api_keys.json"


def _mask(value: str) -> str:
    """Mask a secret to first2…last4 (or <redacted> if too short)."""
    v = str(value).strip()
    if len(v) <= 6:
        return "<redacted>"
    return f"{v[:2]}…{v[-4:]}"


@dataclass
class StoredKey:
    """A stored API key entry (never exposes the raw key)."""

    provider_id: str
    fingerprint: str  # masked: sk-…x4Kp
    base_url: str
    protocol: str
    models: list[str] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "provider_id": self.provider_id,
            "fingerprint": self.fingerprint,
            "base_url": self.base_url,
            "protocol": self.protocol,
            "models": self.models,
        }


class KeyStore:
    """Secure storage for API keys. File-based with 600 permissions."""

    def __init__(self, path: Path | None = None):
        self._path = path or _default_store_path()
        self._data: dict[str, dict[str, Any]] = {}
        self._load()

    def _load(self) -> None:
        if not self._path.exists():
            return
        try:
            self._data = json.loads(
                self._path.read_text(encoding="utf-8", errors="replace")
            )
        except (OSError, ValueError):
            self._data = {}

    def _save(self) -> None:
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._path.write_text(
            json.dumps(self._data, indent=2, ensure_ascii=False),
            encoding="utf-8",
        )
        try:
            os.chmod(self._path, stat.S_IRUSR | stat.S_IWUSR)
        except OSError:
            pass

    def store(
        self,
        provider_id: str,
        api_key: str,
        base_url: str,
        protocol: str,
        models: list[str] | None = None,
    ) -> StoredKey:
        """Store an API key. Returns a StoredKey with masked fingerprint."""
        self._data[provider_id] = {
            "key": api_key,
            "base_url": base_url,
            "protocol": protocol,
            "models": models or [],
        }
        self._save()
        return StoredKey(
            provider_id=provider_id,
            fingerprint=_mask(api_key),
            base_url=base_url,
            protocol=protocol,
            models=models or [],
        )

    def get_key(self, provider_id: str) -> str | None:
        """Retrieve the raw API key for a provider. Use with caution."""
        entry = self._data.get(provider_id)
        if entry and isinstance(entry, dict):
            return entry.get("key")
        return None

    def get_stored(self, provider_id: str) -> StoredKey | None:
        """Get a StoredKey (masked) for a provider."""
        entry = self._data.get(provider_id)
        if not entry or not isinstance(entry, dict):
            return None
        key = entry.get("key", "")
        return StoredKey(
            provider_id=provider_id,
            fingerprint=_mask(key),
            base_url=entry.get("base_url", ""),
            protocol=entry.get("protocol", "openai"),
            models=entry.get("models", []),
        )

    def has_key(self, provider_id: str) -> bool:
        return provider_id in self._data and isinstance(self._data[provider_id], dict)

    def remove(self, provider_id: str) -> bool:
        if provider_id in self._data:
            del self._data[provider_id]
            self._save()
            return True
        return False

    def stored_provider_ids(self) -> list[str]:
        return list(self._data.keys())

    def all_stored(self) -> list[StoredKey]:
        result = []
        for pid in self._data:
            sk = self.get_stored(pid)
            if sk:
                result.append(sk)
        return result
