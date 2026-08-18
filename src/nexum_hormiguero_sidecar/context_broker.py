"""Cartero — Context Broker tipado (OMEGA Fase 13, ciclo 6b).

Preserva el CONCEPTO Cartero (preparación y entrega tipada de contexto) sin
portar el Cartero legacy de packages/core. Opera sobre rutas/planes validados.

Contratos duros:
- solo contexto PERMITIDO por el modo (FAST mínimo / MEDIUM sesión acotada /
  COMPLEX extendido expresamente autorizado);
- budgets por caracteres con truncation_reason registrado;
- dedup determinístico;
- filtro de secretos SIEMPRE (nunca entrega material sensible);
- jamás lee MemoryGateway directo (los ítems llegan ya autorizados por el
  caller, que es quien tiene el contrato con Memory);
- evidencia sin contenido privado (solo conteos/razones/hashes).
"""

from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass, field
from enum import Enum

from .planning.routes import RouteClass


class ContextMode(str, Enum):
    FAST = "fast"
    MEDIUM = "medium"
    COMPLEX = "complex"


# route → modo de contexto. Nada carga "toda la memoria".
ROUTE_CONTEXT_MODE: dict[RouteClass, ContextMode] = {
    RouteClass.LOCAL_FAST: ContextMode.FAST,
    RouteClass.LOCAL_PLAN: ContextMode.MEDIUM,
    RouteClass.TOOL_INTENT: ContextMode.MEDIUM,
    RouteClass.MEMORY_INTENT: ContextMode.MEDIUM,
    RouteClass.PREMIUM_REASONING: ContextMode.COMPLEX,
}

# Budgets por modo (chars). COMPLEX es "extendido autorizado", no infinito.
MODE_BUDGET_CHARS: dict[ContextMode, int] = {
    ContextMode.FAST: 500,
    ContextMode.MEDIUM: 4_000,
    ContextMode.COMPLEX: 16_000,
}

# Patrones de secretos: si un ítem matchea, se EXCLUYE entero (fail-closed;
# no se intenta "limpiar" un secreto a medias).
_SECRET_RE = re.compile(
    r"(sk-[a-zA-Z0-9]{8,}|api[_-]?key|authorization:\s*bearer|x-nexum-token"
    r"|BEGIN [A-Z ]*PRIVATE KEY|password\s*[:=]|contrase[ñn]a\s*[:=]"
    r"|token\s*[:=]\s*\S{8,})",
    re.I,
)


@dataclass
class ContextItem:
    """Un ítem candidato a contexto, YA autorizado por su dueño (scope)."""

    source_id: str  # código corto del origen (ej "session:last", "project:readme")
    scope: str  # "session" | "project" | "user_authorized"
    text: str
    evidence_ref: str = ""


@dataclass
class BrokeredContext:
    mode: ContextMode
    items_included: list[ContextItem] = field(default_factory=list)
    evidence_refs: list[str] = field(default_factory=list)
    total_chars: int = 0
    truncation_reason: str = ""  # "" | "budget_chars" | "item_limit"
    excluded_secret_items: int = 0
    excluded_scope_items: int = 0
    deduped_items: int = 0

    def payload_text(self) -> str:
        return "\n\n".join(i.text for i in self.items_included)

    def public_evidence(self) -> dict:
        """Evidencia segura: conteos y razones, jamás contenido."""
        return {
            "mode": self.mode.value,
            "items": len(self.items_included),
            "total_chars": self.total_chars,
            "truncation_reason": self.truncation_reason,
            "excluded_secret_items": self.excluded_secret_items,
            "excluded_scope_items": self.excluded_scope_items,
            "deduped_items": self.deduped_items,
            "items_hash": hashlib.sha256(self.payload_text().encode()).hexdigest()[:16],
        }


# Scopes admitidos por modo. FAST no arrastra proyecto; COMPLEX exige que el
# ítem de usuario venga explícitamente autorizado (scope user_authorized).
_MODE_SCOPES: dict[ContextMode, frozenset[str]] = {
    ContextMode.FAST: frozenset({"session"}),
    ContextMode.MEDIUM: frozenset({"session", "project"}),
    ContextMode.COMPLEX: frozenset({"session", "project", "user_authorized"}),
}

_MAX_ITEMS = 32


def broker_context(
    route: RouteClass,
    items: list[ContextItem],
    max_items: int = _MAX_ITEMS,
) -> BrokeredContext:
    """Selecciona, filtra, deduplica y acota el contexto para la ruta dada.

    Determinístico y total: nunca lanza por contenido; los ítems inválidos se
    excluyen y se cuentan.
    """
    mode = ROUTE_CONTEXT_MODE.get(route, ContextMode.FAST)
    budget = MODE_BUDGET_CHARS[mode]
    allowed_scopes = _MODE_SCOPES[mode]
    out = BrokeredContext(mode=mode)

    seen_hashes: set[str] = set()
    for item in items:
        # 1. Scope: fuera del modo ⇒ excluido (jamás "por las dudas").
        if item.scope not in allowed_scopes:
            out.excluded_scope_items += 1
            continue
        # 2. Secretos: matchea ⇒ ítem entero afuera, fail-closed.
        if _SECRET_RE.search(item.text):
            out.excluded_secret_items += 1
            continue
        # 3. Dedup por hash del texto normalizado.
        h = hashlib.sha256(" ".join(item.text.split()).encode()).hexdigest()
        if h in seen_hashes:
            out.deduped_items += 1
            continue
        seen_hashes.add(h)
        # 4. Budgets: chars e ítems.
        if len(out.items_included) >= max_items:
            out.truncation_reason = "item_limit"
            break
        item_len = len(item.text)
        if out.total_chars + item_len > budget:
            # Compactar: si entra un prefijo útil (>200 chars), truncar el
            # ítem; si no, cortar acá.
            remaining = budget - out.total_chars
            if remaining > 200:
                truncated = ContextItem(
                    source_id=item.source_id,
                    scope=item.scope,
                    text=item.text[:remaining],
                    evidence_ref=item.evidence_ref,
                )
                out.items_included.append(truncated)
                out.total_chars += remaining
                if truncated.evidence_ref:
                    out.evidence_refs.append(truncated.evidence_ref)
            out.truncation_reason = "budget_chars"
            break
        out.items_included.append(item)
        out.total_chars += item_len
        if item.evidence_ref:
            out.evidence_refs.append(item.evidence_ref)
    return out
