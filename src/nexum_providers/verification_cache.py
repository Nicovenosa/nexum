"""Cache de verificación de credenciales, indexado por fingerprint y con TTL.

Motivo: verificar una credencial puede costar una llamada real al proveedor. Sin
cache, cada `reconcile` —y el refresh `r` del panel se dispara a mano— lanzaría
una ráfaga de requests, con costo y con riesgo de rate limit. Con cache, una
credencial que ya fue verificada no se vuelve a probar hasta que el TTL vence o
la credencial cambia.

Indexar por **fingerprint** (no por provider) hace que el cache se invalide solo
cuando la credencial cambia: si rotás la key, el fingerprint cambia y la entrada
vieja deja de aplicar, sin necesidad de limpiar nada.

Seguridad: acá NO se guarda ninguna credencial. Sólo el sha256 truncado, que
sirve para comparar pero no para reconstruir el valor.

Stdlib only.
"""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

# 6 horas: suficiente para que una sesión de trabajo entera use un solo probe
# por credencial, y corto para que una key revocada no quede "válida" un día.
DEFAULT_TTL_SECS = 6 * 3600

SCHEMA_VERSION = 1


def fingerprint(secret: str) -> str:
    """Identificador estable de una credencial que no permite reconstruirla."""
    return hashlib.sha256(secret.encode("utf-8")).hexdigest()[:16]


@dataclass(frozen=True)
class VerificationEntry:
    provider_id: str
    fingerprint: str
    state: str
    detail: str
    store: str
    verified_at: float

    def expired(self, ttl: float, now: float | None = None) -> bool:
        return (time.time() if now is None else now) - self.verified_at >= ttl

    def to_json(self) -> dict[str, Any]:
        return {
            "provider_id": self.provider_id,
            "fingerprint": self.fingerprint,
            "state": self.state,
            "detail": self.detail,
            "store": self.store,
            "verified_at": self.verified_at,
        }


class VerificationCache:
    """Persistencia best-effort: si el archivo se corrompe, se empieza de cero."""

    def __init__(self, path: Path | None = None, ttl_secs: float = DEFAULT_TTL_SECS):
        self.path = path or self._default_path()
        self.ttl_secs = ttl_secs
        self._entries: dict[str, VerificationEntry] = {}
        self._load()

    @staticmethod
    def _default_path() -> Path:
        cache = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
        return Path(cache) / "nexum/providers/verification-cache.json"

    @staticmethod
    def _key(provider_id: str, fp: str) -> str:
        return f"{provider_id}:{fp}"

    def _load(self) -> None:
        try:
            doc = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return
        if not isinstance(doc, dict) or doc.get("schema_version") != SCHEMA_VERSION:
            return
        for raw in doc.get("entries", []):
            if not isinstance(raw, dict):
                continue
            try:
                entry = VerificationEntry(
                    provider_id=str(raw["provider_id"]),
                    fingerprint=str(raw["fingerprint"]),
                    state=str(raw["state"]),
                    detail=str(raw.get("detail", "")),
                    store=str(raw.get("store", "")),
                    verified_at=float(raw["verified_at"]),
                )
            except (KeyError, TypeError, ValueError):
                continue
            self._entries[self._key(entry.provider_id, entry.fingerprint)] = entry

    def get(self, provider_id: str, secret: str) -> VerificationEntry | None:
        """Entrada vigente para esa credencial exacta, o None si no hay o venció."""
        entry = self._entries.get(self._key(provider_id, fingerprint(secret)))
        if entry is None or entry.expired(self.ttl_secs):
            return None
        return entry

    def put(
        self, provider_id: str, secret: str, state: str, detail: str, store: str
    ) -> VerificationEntry:
        entry = VerificationEntry(
            provider_id=provider_id,
            fingerprint=fingerprint(secret),
            state=state,
            detail=detail,
            store=store,
            verified_at=time.time(),
        )
        self._entries[self._key(provider_id, entry.fingerprint)] = entry
        return entry

    def save(self) -> bool:
        """Escritura atómica. Un fallo no rompe el reconcile: sólo se pierde cache."""
        payload = {
            "schema_version": SCHEMA_VERSION,
            "ttl_secs": self.ttl_secs,
            "entries": [e.to_json() for e in self._entries.values()],
        }
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            fd, tmp = tempfile.mkstemp(
                dir=str(self.path.parent), prefix=".verification-", suffix=".json"
            )
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                json.dump(payload, handle, indent=2)
                handle.write("\n")
            os.replace(tmp, self.path)
            os.chmod(self.path, 0o600)
            return True
        except OSError:
            return False
