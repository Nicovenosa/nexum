"""Canario de ejecución POR MODELO.

El catálogo declaraba `claude-opus-4-8` usable y el puente contestaba 404 al
chatear. Nadie lo detectaba: Doctor valida forma, el registry gate valida
correspondencia catálogo↔registry, y **ninguno de los dos toca al proveedor**.
Toda la maquinaria de verificación se detenía en el borde del sistema.

La verificación de credencial que ya existe no alcanza: la credencial de
`claude_code` es válida —el puente la acepta— y es el MODELO el que no está.
Son dos preguntas distintas y sólo una tenía respuesta.

# Barato por construcción

Un canario que prueba todos los modelos de todos los providers en cada
`reconcile` es una ráfaga: con 65 modelos declarados serían 65 requests cada vez
que alguien abre el panel. Tres límites lo evitan:

1. **Cache con TTL** (6 h por default, el mismo que la verificación de
   credencial). Un modelo probado no se vuelve a probar hasta que vence.
2. **Presupuesto por corrida** (`max_probes`): aunque haya 65 sin probar, se
   prueban unos pocos y el resto queda para la próxima. La cobertura se completa
   sola en varias corridas en vez de golpear de una.
3. **Sólo providers usables.** Probar el modelo de un provider sin credencial no
   dice nada del modelo.

El costo de cada probe es una completion con `max_tokens=1`.

# Qué NO hace

No marca un modelo como malo por un error de red o un 429: eso es estado del
momento, no del modelo. Sólo `not_found` es una afirmación sobre el modelo en
sí, y es la única que se persiste como fallo.
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

DEFAULT_TTL_SECS = 6 * 3600
DEFAULT_MAX_PROBES = 3

# Veredictos
OK = "ok"
NOT_FOUND = "not_found"
UNKNOWN = "unknown"


@dataclass
class CanaryEntry:
    verdict: str
    detail: str
    checked_at: float = field(default_factory=time.time)

    def expired(self, ttl: float, now: float | None = None) -> bool:
        return (time.time() if now is None else now) - self.checked_at >= ttl

    def to_json(self) -> dict[str, Any]:
        return {
            "verdict": self.verdict,
            "detail": self.detail,
            "checked_at": self.checked_at,
        }

    @classmethod
    def from_json(cls, raw: dict[str, Any]) -> "CanaryEntry":
        return cls(
            verdict=str(raw.get("verdict", UNKNOWN)),
            detail=str(raw.get("detail", "")),
            checked_at=float(raw.get("checked_at", 0.0)),
        )


class ModelCanaryCache:
    """Resultados por (provider, modelo), con TTL y persistencia atómica."""

    def __init__(self, path: Path | None = None, ttl_secs: float = DEFAULT_TTL_SECS):
        self.path = path if path is not None else self._default_path()
        self.ttl_secs = ttl_secs
        self._entries: dict[str, CanaryEntry] = {}
        self._load()

    @staticmethod
    def _default_path() -> Path:
        import os

        cache = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
        return Path(cache) / "nexum/providers/model-canary.json"

    @staticmethod
    def _key(provider_id: str, model_id: str) -> str:
        return f"{provider_id}::{model_id}"

    def _load(self) -> None:
        try:
            raw = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return
        if not isinstance(raw, dict):
            return
        for key, value in raw.items():
            if isinstance(value, dict):
                self._entries[key] = CanaryEntry.from_json(value)

    def save(self) -> None:
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            tmp = self.path.with_suffix(".tmp")
            tmp.write_text(
                json.dumps(
                    {k: v.to_json() for k, v in self._entries.items()},
                    indent=2,
                    ensure_ascii=False,
                )
                + "\n",
                encoding="utf-8",
            )
            tmp.replace(self.path)
        except OSError:
            # Un canario que rompe el reconcile es peor que no tenerlo.
            pass

    def get(self, provider_id: str, model_id: str) -> CanaryEntry | None:
        entry = self._entries.get(self._key(provider_id, model_id))
        if entry is None or entry.expired(self.ttl_secs):
            return None
        return entry

    def put(self, provider_id: str, model_id: str, entry: CanaryEntry) -> None:
        self._entries[self._key(provider_id, model_id)] = entry


def _verdict_from_status(status: int | None, detail: str) -> tuple[str, str]:
    """Traduce un HTTP a veredicto SOBRE EL MODELO.

    Sólo el 404 dice algo del modelo. Un 429 dice que hay cuota agotada ahora, y
    un error de red dice que no había red: ninguno de los dos es una afirmación
    sobre si el modelo existe, y persistirlos como fallo haría que el canario
    mienta al rato siguiente.
    """
    if status is None:
        return UNKNOWN, f"sin respuesta: {detail}"
    if status == 404:
        return NOT_FOUND, "el proveedor no conoce este modelo (404)"
    if 200 <= status < 300:
        return OK, "responde"
    return UNKNOWN, f"HTTP {status}: {detail}"


def probe_models(
    candidatos: list[tuple[str, str, str, str | None]],
    cache: ModelCanaryCache | None = None,
    max_probes: int = DEFAULT_MAX_PROBES,
    probe: Callable[[str, str | None, str], tuple[int | None, str]] | None = None,
) -> dict[str, CanaryEntry]:
    """Prueba hasta `max_probes` modelos sin veredicto fresco.

    `candidatos` son tuplas `(provider_id, model_id, url, api_key)`. Devuelve los
    veredictos de TODOS los candidatos que tengan uno —de cache o recién
    probados—, no sólo los probados en esta corrida.
    """
    store = cache if cache is not None else ModelCanaryCache()
    salida: dict[str, CanaryEntry] = {}
    restantes = max_probes

    for provider_id, model_id, url, api_key in candidatos:
        key = ModelCanaryCache._key(provider_id, model_id)
        cached = store.get(provider_id, model_id)
        if cached is not None:
            salida[key] = cached
            continue
        if restantes <= 0:
            continue
        restantes -= 1
        if probe is None:
            from nexum_providers import credential_verifier, http_client

            payload = dict(credential_verifier._PROBE_PAYLOAD_TEMPLATE, model=model_id)
            result = http_client.request(
                url, api_key=api_key, method="POST", payload=payload, timeout=15.0
            )
            status, detail = result.status, (result.error or result.body or "")
        else:
            status, detail = probe(url, api_key, model_id)
        verdict, det = _verdict_from_status(status, detail)
        entry = CanaryEntry(verdict=verdict, detail=det)
        store.put(provider_id, model_id, entry)
        salida[key] = entry

    store.save()
    return salida
