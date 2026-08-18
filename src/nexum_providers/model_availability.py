"""Disponibilidad POR MODELO, no sólo por provider.

El modelo de datos anterior asumía un provider = un estado. OpenCode Zen lo
rompe: sirve 7 modelos gratis sin credencial y 52 de pago con api-key, en el
mismo endpoint. Para representarlo hace falta estado a nivel modelo.

Cómo se decide qué es gratis, en orden de costo:

  1. **Pricing declarado** (`model_catalog_sources`): el catálogo de la CLI trae
     `cost` por modelo. `cost.input == 0 and cost.output == 0` arma el conjunto
     candidato. Verificado el 2026-07-26: la intersección de ese conjunto con
     los modelos vivos da EXACTAMENTE los 7 que muestra la CLI.
  2. **Un canario**, no siete: `cost == 0` es heurística, no hecho — un modelo
     podría costar cero y exigir auth igual. Se confirma con UNA llamada sin
     credencial sobre un solo modelo del conjunto. La heurística arma el
     conjunto; la verificación confirma que el camino es real.
  3. Sin fuente de pricing NO se adivina: el provider queda en
     `unknown_availability` con el motivo escrito. Mostrar todos como usables
     sería afirmar sin verificar; mostrar ninguno sería esconder el provider.

Stdlib only.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Mapping

# Disponibilidad de un modelo individual.
MODEL_FREE = "free"  # servido sin credencial
MODEL_PAID_OK = "paid_available"  # de pago y la credencial habilita
MODEL_PAID_BLOCKED = "paid_blocked"  # de pago, la credencial no habilita
MODEL_UNKNOWN = "unknown"  # no se pudo determinar

# Estado del provider cuando no hay de dónde sacar el pricing.
UNKNOWN_AVAILABILITY = "unknown_availability"
UNKNOWN_AVAILABILITY_DETAIL = (
    "no se pudo determinar cuáles modelos son gratuitos: falta el catálogo de precios"
)


@dataclass(frozen=True)
class ModelSplit:
    """Partición de los modelos vivos de un provider según su pricing."""

    free: list[str] = field(default_factory=list)
    paid: list[str] = field(default_factory=list)
    source: str = ""  # de dónde salió el pricing (para mostrar el origen)
    determined: bool = False

    @property
    def total(self) -> int:
        return len(self.free) + len(self.paid)


def _expand(path: str, env: Mapping[str, str]) -> Path:
    home = Path(env.get("HOME") or Path.home())
    cache = env.get("XDG_CACHE_HOME") or str(home / ".cache")
    data = env.get("XDG_DATA_HOME") or str(home / ".local/share")
    return Path(
        path.replace("$XDG_CACHE_HOME", cache)
        .replace("$XDG_DATA_HOME", data)
        .replace("~", str(home), 1)
    )


def _display(path: Path, env: Mapping[str, str]) -> str:
    home = str(env.get("HOME") or Path.home())
    text = str(path)
    return "~" + text[len(home) :] if text.startswith(home) else text


def load_pricing(
    source: Any, env: Mapping[str, str]
) -> tuple[dict[str, Any], str] | None:
    """Pricing por modelo desde un `ModelCatalogSource`, o None si no está."""
    if getattr(source, "kind", "") != "local_cache" or not source.path:
        return None
    path = _expand(source.path, env)
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if not isinstance(doc, dict):
        return None
    entry = doc.get(source.namespace) if source.namespace else doc
    if not isinstance(entry, dict):
        return None
    models = entry.get("models")
    return (models, _display(path, env)) if isinstance(models, dict) else None


def is_free(model_meta: Any) -> bool:
    """`cost.input == 0 and cost.output == 0`. Heurística, confirmada por canario."""
    if not isinstance(model_meta, dict):
        return False
    cost = model_meta.get("cost")
    if not isinstance(cost, dict):
        return False
    return cost.get("input") == 0 and cost.get("output") == 0


def split_models(
    definition: Any, live_models: list[str], env: Mapping[str, str] | None = None
) -> ModelSplit:
    """Parte los modelos vivos en gratis y de pago según el pricing declarado.

    Las fuentes se recorren en orden de precedencia; la primera que resuelve,
    gana. Si ninguna resuelve, `determined=False` y el llamador debe publicar
    `unknown_availability` en vez de adivinar.
    """
    environment = dict(os.environ if env is None else env)
    for source in getattr(definition, "model_catalog_sources", ()) or ():
        loaded = load_pricing(source, environment)
        if loaded is None:
            continue
        pricing, origen = loaded
        free = [m for m in live_models if is_free(pricing.get(m))]
        paid = [m for m in live_models if m not in free]
        return ModelSplit(free=free, paid=paid, source=origen, determined=True)
    return ModelSplit(free=[], paid=list(live_models), determined=False)


def free_canary(split: ModelSplit) -> str | None:
    """UN modelo con el cual confirmar el camino sin credencial.

    Uno, no siete: probar cada modelo del conjunto gastaría N llamadas para
    demostrar lo mismo — que el endpoint responde sin Authorization.
    """
    return split.free[0] if split.free else None
