"""Installed, safe provider-catalog reconciliation for Nexum R2.

The historical pipeline was doctor -> resolver -> catalog. This module keeps
that shape with local, redacted intermediate metadata in XDG cache, while the
installed base registry guarantees that a failed source never removes support.
It never starts OAuth and never writes under ``~/.peri``.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
import shutil
import socket
import sys
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping

from nexum_providers import (
    credential_verifier,
    http_client,
    model_availability,
    provider_resolve,
)
from nexum_providers.catalog_gen.native_login_detector import detect_native_login
from nexum_providers.catalog_gen.provider_registry import PROVIDER_REGISTRY
from nexum_providers.catalog_gen.reserved_models_lib import (
    ReservedPolicyError,
    reserved_model_names,
)
from nexum_providers.key_store import KeyStore
from nexum_providers.store_reader import resolve_from_stores
from nexum_providers.verification_cache import VerificationCache


SOURCE_BASE_REGISTRY = "base_registry"
SOURCE_NATIVE_CLIS = "native_clis"
SOURCE_NATIVE_LOGINS = "native_logins"
SOURCE_BRIDGE_CLIPROXYAPI = "bridge"
SOURCE_DIRECT_PROVIDER_RESOLVER = "resolver"
SOURCE_DIRECT_KEY_STORES = "direct_key_stores"
SOURCE_LOCAL_OLLAMA = "ollama"
SOURCE_KEYSTORE = "keystore"
SOURCE_ENV_AUTOLOGIN = "env_autologin"
SOURCE_PREVIOUS_VALID_SNAPSHOT = "previous_snapshot"

ALL_SOURCES = (
    SOURCE_BASE_REGISTRY,
    SOURCE_NATIVE_CLIS,
    SOURCE_NATIVE_LOGINS,
    SOURCE_BRIDGE_CLIPROXYAPI,
    SOURCE_DIRECT_PROVIDER_RESOLVER,
    SOURCE_DIRECT_KEY_STORES,
    SOURCE_LOCAL_OLLAMA,
    SOURCE_KEYSTORE,
    SOURCE_ENV_AUTOLOGIN,
    SOURCE_PREVIOUS_VALID_SNAPSHOT,
)

# ─── Resolver directo (NEXUM_PROVIDER_CONNECT_V2) ────────────────────────────
#
# La fuente `resolver` nació apagada en 819dc0e como un SourceResult.failed()
# constante, igual que SOURCE_KEYSTORE, que fue encendido en eba71eb. Esto la
# enciende con el mismo patrón, detrás de un flag con default OFF: sin la
# variable, el catálogo resultante es byte-idéntico al baseline.

FLAG_PROVIDER_CONNECT_V2 = "NEXUM_PROVIDER_CONNECT_V2"
LOG_MARKER_PROVIDER_CONNECT_V2 = "NEXUM_MARK_PROVIDER_CONNECT_V2_20260725"
RESOLVER_DISABLED_REASON = "direct resolver requires explicit credential use"


# ─── Estampa de generación (4.1) ─────────────────────────────────────────────
#
# El catálogo declara con qué GENERACIÓN de contrato fue escrito, y el binario
# la compara al leerlo. No es higiene: la guarda de `nexum-acp` concede el
# camino SIN AUTENTICACIÓN leyendo `credential_state == "free_access"` de este
# artefacto. Un catálogo de otra generación puede conceder algo que este
# binario no debería estar concediendo.
#
# NO es el sha del binario: cambiaría en cada build sin que el contrato se
# mueva. Se bumpea sólo cuando cambia lo que el catálogo PROMETE:
#   1  formato R2 original
#   2  + credential_state / connect_kind / credential_store
#   3  + free_access como CONCESIÓN DE ACCESO (la que custodia la guarda)
def _load_catalog_generation() -> int:
    """Generación desde la FUENTE ÚNICA (`config/catalog-contract.json`).

    No se hardcodea acá: si Python y Rust llevaran cada uno su número, el
    mecanismo que detecta discrepancias entre artefactos derivados sería él
    mismo dos artefactos que pueden discrepar. El test cruzado de Rust lee este
    mismo archivo y falla si los dos lados no coinciden.
    """
    root = Path(__file__).resolve().parents[2]
    for base in (
        # Slot instalado: <version_root>/catalog-contract.json, hermano de src/.
        root / "catalog-contract.json",
        # Checkout: <cli_root>/config/catalog-contract.json.
        root / "config/catalog-contract.json",
    ):
        try:
            value = json.loads(base.read_text(encoding="utf-8")).get("generation")
        except (OSError, ValueError):
            continue
        if isinstance(value, int) and value > 0:
            return value
    # Sin el contrato instalado no se inventa una generación: 0 significa
    # "sin estampa" y el binario no concederá acceso libre, avisando.
    return 0


CATALOG_GENERATION = _load_catalog_generation()

# Los catálogos escritos antes de esta estampa no tienen el campo. Se tratan
# como generación 0: no conceden free_access, pero tampoco rompen nada de lo
# que no depende de una concesión.
CATALOG_GENERATION_ABSENT = 0

# Presupuesto de red. urllib aplica UN timeout de socket (no separa connect de
# read), así que se usa el de lectura y se acota además por el presupuesto
# global restante para que el peor caso siga siendo determinista.
RESOLVER_READ_TIMEOUT_SECS = 8.0
# `/api/show` es una consulta por modelo: barata individualmente, pero con un
# presupuesto total para que instalar veinte modelos no alargue el reconcile.
OLLAMA_SHOW_TIMEOUT_SECS = 2.0
OLLAMA_CAPS_BUDGET_SECS = 10.0
RESOLVER_BUDGET_SECS = 20.0

# Canario de volumen: avisa, nunca trunca en silencio.
SECTION_MODEL_WARN_THRESHOLD = 40

# Precedencia ante colisión de model_id entre providers, de mayor a menor.
# El puente usa suscripciones ya pagadas sin costo marginal; el agregador
# cobra por token. Ante empate, gana el más barato para el usuario.
AUTH_MODE_PRECEDENCE = {
    "local_no_auth": 0,
    "cli_oauth": 1,
    "direct_key": 2,
    "bridge_proxy": 3,
    "openai_compatible": 4,
    "static_api_key": 5,
}
_PRECEDENCE_UNKNOWN = 99

_FLAG_TRUTHY = frozenset({"1", "true", "yes", "on"})
_FLAG_FALSY = frozenset({"0", "false", "no", "off"})

_SECRET_FIELDS = frozenset(
    {
        "api_key",
        "access_token",
        "refresh_token",
        "token",
        "secret",
        "password",
        "cookie",
        "email",
    }
)


@dataclass(frozen=True)
class ReconcilePaths:
    """All R2 state is explicit and XDG scoped."""

    live_catalog: Path
    previous_catalog: Path
    status: Path
    cache_dir: Path
    settings: Path

    @classmethod
    def from_environment(cls, env: Mapping[str, str] | None = None) -> "ReconcilePaths":
        values = dict(os.environ if env is None else env)
        home = Path(values.get("HOME") or Path.home())
        data = Path(values.get("XDG_DATA_HOME") or home / ".local/share")
        config = Path(values.get("XDG_CONFIG_HOME") or home / ".config")
        cache = Path(values.get("XDG_CACHE_HOME") or home / ".cache")
        providers = data / "nexum/providers"
        return cls(
            live_catalog=providers / "provider-catalog-live.json",
            previous_catalog=providers / "provider-catalog-live.previous.json",
            status=providers / "provider-reconcile-status.json",
            cache_dir=cache / "nexum/providers",
            settings=config / "nexum/settings.json",
        )


@dataclass(frozen=True)
class SourceResult:
    available: bool
    updates: dict[str, dict[str, Any]]
    warning: str | None = None

    @classmethod
    def ok(cls, updates: dict[str, dict[str, Any]]) -> "SourceResult":
        return cls(True, updates)

    @classmethod
    def failed(cls, warning: str) -> "SourceResult":
        return cls(False, {}, warning)


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _safe(value: Any) -> Any:
    """Drop secret-bearing fields recursively before any persistence or output."""
    if isinstance(value, dict):
        return {
            k: _safe(v) for k, v in value.items() if k.lower() not in _SECRET_FIELDS
        }
    if isinstance(value, list):
        return [_safe(item) for item in value]
    return value


def validate_catalog(catalog: Any) -> bool:
    if not isinstance(catalog, dict) or not isinstance(catalog.get("providers"), list):
        return False
    ids: set[str] = set()
    for provider in catalog["providers"]:
        if not isinstance(provider, dict):
            return False
        provider_id = provider.get("provider_id") or provider.get("id")
        if not isinstance(provider_id, str) or not provider_id or provider_id in ids:
            return False
        ids.add(provider_id)
    return True


def load_valid_catalog(path: Path) -> dict[str, Any] | None:
    try:
        candidate = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return candidate if validate_catalog(candidate) else None


def _provider_id(entry: Mapping[str, Any]) -> str:
    value = entry.get("provider_id") or entry.get("id")
    return value if isinstance(value, str) else ""


def _default_provider(definition: Any) -> dict[str, Any]:
    return {
        "id": definition.provider_id,
        "provider_id": definition.provider_id,
        "display_name": definition.display_name,
        "family": definition.family,
        "auth_mode": definition.auth_mode,
        "status": "not_configured",
        "availability": "NOT_CONFIGURED",
        "usable_now": False,
        "models": [model.model_id for model in definition.static_models],
        "models_detected": [model.model_id for model in definition.static_models],
        "models_status": "known_metadata",
        "next_action": "connect",
        "credential_detected": False,
        "native_login_detected": False,
    }


def build_catalog(
    base_catalog: dict[str, Any] | None,
    sources: Mapping[str, SourceResult],
) -> dict[str, Any]:
    """Merge independent redacted source results without losing registry entries."""
    base_available = base_catalog is not None and validate_catalog(base_catalog)
    if base_available:
        providers = {
            _provider_id(provider): copy.deepcopy(provider)
            for provider in base_catalog["providers"]
            if _provider_id(provider)
        }
        # A stale base cannot remove a provider known by the product registry.
        for definition in PROVIDER_REGISTRY.values():
            providers.setdefault(definition.provider_id, _default_provider(definition))
    else:
        providers = {}

    used_sources: list[str] = [SOURCE_BASE_REGISTRY] if base_available else []
    warnings: list[str] = []
    for name, result in sources.items():
        if result.available:
            used_sources.append(name)
        elif result.warning:
            warnings.append(f"{name}: {result.warning}")
        for provider_id, update in result.updates.items():
            if provider_id not in providers:
                providers[provider_id] = {
                    "id": provider_id,
                    "provider_id": provider_id,
                    "display_name": provider_id,
                    "family": provider_id,
                    "status": "unknown",
                    "availability": "UNKNOWN",
                    "usable_now": False,
                    "models": [],
                    "models_detected": [],
                }
            providers[provider_id].update(_safe(update))
            providers[provider_id]["id"] = provider_id
            providers[provider_id]["provider_id"] = provider_id

    missing = [source for source in ALL_SOURCES if source not in used_sources]
    catalog_kind = "complete" if base_available else "partial"
    catalog = {
        "schema_version": 2,
        "version": "2",
        "catalog_version": "2",
        "catalog_kind": catalog_kind,
        "partial_sources": [] if base_available else used_sources,
        "missing_sources": missing,
        "generation_warnings": warnings,
        "generated_at": _now(),
        "providers": list(providers.values()),
        "catalog": (
            copy.deepcopy(base_catalog.get("catalog", []))
            if base_available and isinstance(base_catalog.get("catalog"), list)
            else []
        ),
        "notes": [
            "R2 reconcile: usable_now requiere evidencia de una fuente local o bridge.",
            "Los modelos de metadata conocida no son seleccionables sin usable_now.",
        ],
    }
    if not base_available:
        catalog["generation_warnings"].append(
            "base registry unavailable; catalog is partial"
        )
    return _safe(catalog)


def _atomic_write(path: Path, data: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = (json.dumps(data, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    fd, temp_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp_name, path)
        directory_fd = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    except BaseException:
        try:
            os.unlink(temp_name)
        except FileNotFoundError:
            pass
        raise


def publish_catalog(paths: ReconcilePaths, catalog: dict[str, Any]) -> None:
    if not validate_catalog(catalog):
        raise ValueError("catalog schema rejected before publish")
    current = load_valid_catalog(paths.live_catalog)
    if current is not None:
        _atomic_write(paths.previous_catalog, current)
    _atomic_write(paths.live_catalog, catalog)


def _native_cli_source() -> SourceResult:
    executable_for = {
        "claude_code": "claude",
        "codex_cli": "codex",
        "gemini_cli": "gemini",
        "opencode_zen": "opencode",
        "opencode_go": "opencode",
        "mimo_code": "mimo",
        "ollama_local": "ollama",
    }
    updates = {
        provider_id: {"installed": True, "availability": "INSTALLED"}
        for provider_id, binary in executable_for.items()
        if shutil.which(binary)
    }
    return SourceResult.ok(updates)


def _native_login_source() -> SourceResult:
    updates: dict[str, dict[str, Any]] = {}
    for definition in PROVIDER_REGISTRY.values():
        try:
            detected = detect_native_login(definition).detected
        except OSError:
            continue
        if detected:
            updates[definition.provider_id] = {
                "native_login_detected": True,
                "credential_detected": True,
                "availability": "LOGIN_DETECTED",
                "status": "detected_login",
                "usable_now": False,
            }
    return SourceResult.ok(updates)


def _bridge_source() -> SourceResult:
    # This R2 probe only observes a loopback port. It does not ensure/start the
    # service and never requests an OAuth flow.
    try:
        with socket.create_connection(("127.0.0.1", 8317), timeout=0.5):
            running = True
    except OSError:
        running = False
    if not running:
        return SourceResult.failed("CLIProxyAPI unavailable")
    return SourceResult.ok(
        {
            provider_id: {
                "bridge_status": "bridge_running",
                "availability": "LOGIN_DETECTED",
                "usable_now": False,
            }
            for provider_id in ("claude_code", "codex_cli", "gemini_cli")
        }
    )


def _ollama_source() -> SourceResult:
    """Ollama local: puerto abierto ⇒ usable, y los modelos salen de su /models.

    Antes esta fuente sólo marcaba `usable_now` y los modelos venían de la lista
    ESTÁTICA del catálogo base. Consecuencia medida el 2026-07-26: el usuario
    tenía 8 modelos instalados y `/modelo` mostraba 4, los de una generación
    vieja — los tres `minicpm5*` no aparecían por ningún lado y ninguna política
    los ocultaba. Un modelo sólo se oculta si hay una razón escrita.
    """
    try:
        with socket.create_connection(("127.0.0.1", 11434), timeout=0.5):
            pass
    except OSError:
        return SourceResult.failed("Ollama unavailable")

    update: dict[str, Any] = {
        "status": "detected_login",
        "availability": "LOCAL_PROVIDER_USABLE",
        "usable_now": True,
        "models_status": "unproven",
    }

    payload = _probe_models_endpoint(
        "http://127.0.0.1:11434/v1", "ollama", RESOLVER_READ_TIMEOUT_SECS
    )
    reserved = _reserved_models()
    live = models_for_owner(payload, None, set())
    if live:
        user_facing = [m for m in live if m not in reserved]
        reserved_here = [m for m in live if m in reserved]
        update.update(
            {
                "models": user_facing,
                "models_detected": list(live),
                "models_status": "probed_live",
                "model_policy": {
                    "user_facing_models": user_facing,
                    "user_facing_count": len(user_facing),
                    "reserved_internal_models": reserved_here,
                    "reserved_internal_count": len(reserved_here),
                    "all_models_reserved": bool(live) and not user_facing,
                    "selectable_models": user_facing,
                    "policy": (
                        "internal_only models are filtered from /modelo; "
                        "Hormiguero reads them directly from Ollama."
                    ),
                },
            }
        )
        # Capacidad de herramientas declarada por modelo. Sin esto, un modelo
        # que no sabe usar tools muele el tope entero de vueltas antes de que
        # alguien se entere; con esto el runtime falla de entrada y con mensaje.
        caps = _ollama_tool_capabilities(list(live), OLLAMA_CAPS_BUDGET_SECS)
        if caps:
            update["model_capabilities"] = {
                model: {"tools": supports} for model, supports in caps.items()
            }
    return SourceResult.ok({"ollama_local": update})


def _ollama_tool_capabilities(models: list[str], budget: float) -> dict[str, bool]:
    """¿Cada modelo local declara saber usar herramientas?

    Sale de `/api/show` de Ollama, que publica `capabilities`. **No se infiere
    del nombre del modelo**: `moondream` no declara `tools` y `qwen2.5:0.5b` sí,
    y no hay regla de nomenclatura que prediga eso.

    Un modelo ausente del dict es un modelo del que no sabemos nada — y no saber
    NO es lo mismo que no poder. Sólo se declara lo que el proveedor afirma.
    """
    caps: dict[str, bool] = {}
    for model in models:
        if budget <= 0:
            break
        started = time.monotonic()
        result = http_client.request(
            "http://127.0.0.1:11434/api/show",
            method="POST",
            payload={"model": model},
            timeout=max(0.1, min(OLLAMA_SHOW_TIMEOUT_SECS, budget)),
        )
        budget -= time.monotonic() - started
        if not result.ok:
            continue
        payload = result.json()
        if not isinstance(payload, dict):
            continue
        declared = payload.get("capabilities")
        if isinstance(declared, list):
            caps[model] = "tools" in declared
    return caps


def _env_source(env: Mapping[str, str]) -> SourceResult:
    updates: dict[str, dict[str, Any]] = {}
    for definition in PROVIDER_REGISTRY.values():
        if any(env.get(name, "").strip() for name in definition.env_vars):
            updates[definition.provider_id] = {
                "credential_detected": True,
                "availability": "AUTH_PRESENT",
                "status": "probe_pending",
                "usable_now": False,
                "models_status": "probe_pending",
            }
    return SourceResult.ok(updates)


def _keystore_source(paths: ReconcilePaths) -> SourceResult:
    """Expose a stored credential without upgrading it to usable without a probe."""
    store = KeyStore(paths.live_catalog.parent / "api_keys.json")
    prior = load_valid_catalog(paths.live_catalog) or {}
    prior_by_id = {
        _provider_id(entry): entry
        for entry in prior.get("providers", [])
        if isinstance(entry, dict) and _provider_id(entry)
    }
    updates: dict[str, dict[str, Any]] = {}
    for stored in store.all_stored():
        if stored.provider_id not in PROVIDER_REGISTRY:
            continue
        previous = prior_by_id.get(stored.provider_id, {})
        models = [
            model for model in previous.get("models", []) if isinstance(model, str)
        ]
        if not models:
            models = list(stored.models)
        update: dict[str, Any] = {
            "credential_detected": True,
            "availability": "AUTH_PRESENT",
            "status": "credential_detected",
            "usable_now": False,
            "models": models,
            "models_detected": models,
            "models_status": "stored_credential_unproven",
        }
        if previous.get("usable_now") is True:
            update.update(
                {
                    "availability": "KEYSTORE_VALIDATED_SNAPSHOT",
                    "status": previous.get("status", "usable"),
                    "usable_now": True,
                    "models_status": previous.get("models_status", "ok"),
                }
            )
        updates[stored.provider_id] = update
    return SourceResult.ok(updates)


def flag_enabled(env: Mapping[str, str]) -> bool:
    """¿El resolver de providers está habilitado? **Default ON.**

    Nació como opt-in explícito, cuando el resolver era nuevo y el riesgo estaba
    en encenderlo. Hoy el riesgo está en lo contrario: sin el flag, el catálogo
    publica un solo provider usable y `/modelo` queda casi vacío sobre una
    máquina que tiene seis providers con credencial válida. Un default que
    esconde lo que el sistema ya sabe hacer no es un resguardo.

    La variable queda como **interruptor para apagar**, no para prender::

        NEXUM_PROVIDER_CONNECT_V2=0   # vuelve al catálogo sin resolver

    Ausente o con cualquier valor no reconocido como apagado ⇒ encendido. Falla
    hacia el comportamiento sano: un typo no deja al usuario sin modelos.
    """
    return env.get(FLAG_PROVIDER_CONNECT_V2, "").strip().lower() not in _FLAG_FALSY


def _emit_marker() -> None:
    """Marcador único del path modificado (contrato §3.4: sin esto no se reporta)."""
    print(LOG_MARKER_PROVIDER_CONNECT_V2, file=sys.stderr)


def _reserved_models() -> set[str]:
    """Política de reservados; ante error se usa el baseline, nunca un set vacío."""
    try:
        return reserved_model_names()
    except ReservedPolicyError:
        from nexum_providers.catalog_gen.reserved_models_lib import (
            BASELINE_RESERVED_MODELS,
        )

        return set(BASELINE_RESERVED_MODELS)


def models_for_owner(
    payload: Any, owner: str | None, reserved: set[str] | None = None
) -> list[str]:
    """Modelos de un `owned_by`, deduplicados, sin reservados y a prueba de basura.

    `owner=None` significa «todos los modelos del endpoint»: lo usan los
    providers con endpoint propio, donde no hay que particionar nada.

    El endpoint es de terceros: cualquier ítem que no sea un dict con `id`
    string se descarta en silencio en vez de romper el reconcile entero.
    """
    blocked = reserved or set()
    out: list[str] = []
    if not isinstance(payload, list):
        return out
    for item in payload:
        if not isinstance(item, dict):
            continue
        model_id = item.get("id")
        if not isinstance(model_id, str) or not model_id:
            continue
        if owner is not None and item.get("owned_by") != owner:
            continue
        if model_id in blocked or model_id in out:
            continue
        out.append(model_id)
    return out


def apply_precedence(
    updates: Mapping[str, dict[str, Any]],
) -> tuple[dict[str, dict[str, Any]], list[str]]:
    """Resuelve colisiones de model_id entre providers por `auth_mode`.

    El provider de mayor precedencia se queda el modelo; los demás lo pierden de
    su sección. Devuelve `(updates, warnings)`.
    """

    def rank(provider_id: str) -> int:
        definition = PROVIDER_REGISTRY.get(provider_id)
        if definition is None:
            return _PRECEDENCE_UNKNOWN
        return AUTH_MODE_PRECEDENCE.get(definition.auth_mode, _PRECEDENCE_UNKNOWN)

    result = {pid: dict(update) for pid, update in updates.items()}
    claimed: dict[str, str] = {}
    warnings: list[str] = []
    for provider_id in sorted(result, key=lambda p: (rank(p), p)):
        kept: list[str] = []
        for model_id in result[provider_id].get("models", []):
            owner = claimed.get(model_id)
            if owner is None:
                claimed[model_id] = provider_id
                kept.append(model_id)
            else:
                warnings.append(
                    f"modelo '{model_id}' servido por '{owner}' y '{provider_id}'; "
                    f"se conserva en '{owner}' por precedencia"
                )
        result[provider_id]["models"] = kept
    return result, warnings


def _probe_models_endpoint(
    base_url: str, api_key: str, budget_left: float
) -> list[Any] | None:
    """GET {base_url}/models autenticado. `None` = no promovible.

    La api key viaja sólo en el header y jamás se loguea. Cualquier fallo
    (red, HTTP != 200, JSON inválido, schema raro) devuelve None: el provider
    conserva el estado que ya tenía en vez de degradarse.
    """
    url = base_url.rstrip("/") + "/models"
    timeout = max(0.1, min(RESOLVER_READ_TIMEOUT_SECS, budget_left))
    # Vía http_client: el User-Agent declarado NO es opcional. `opencode.ai`
    # devuelve 403 al UA por defecto de Python, y un probe sin UA concluiría
    # "credencial inválida" cuando el problema es el filtro del proveedor.
    result = http_client.request(url, api_key=api_key, timeout=timeout)
    if not result.ok:
        return None
    payload = result.json()
    if isinstance(payload, list):
        return payload
    if isinstance(payload, dict):
        data = payload.get("data")
        return data if isinstance(data, list) else None
    return None


def _default_resolver(provider_id: str, env: Mapping[str, str]) -> dict[str, Any]:
    return provider_resolve.resolve(provider_id, env=env)


def _resolver_source(
    paths: ReconcilePaths,
    env: Mapping[str, str],
    *,
    resolver: Any = None,
    prober: Any = None,
) -> SourceResult:
    """Promueve a `usable_now` los providers cuya credencial prueba modelos vivos.

    Elegibles: los que declaran `bridge_model_owner` en el registry. Ningún id
    de provider ni de modelo aparece en esta función (contrato §3.5).

    Garantía de no-degradación: si algo falla, el provider simplemente no
    aparece en los updates, así que conserva el estado que ya tenía.
    """
    if not flag_enabled(env):
        return SourceResult.failed(RESOLVER_DISABLED_REASON)
    _emit_marker()

    resolve_fn = resolver or _default_resolver
    probe_fn = prober or _probe_models_endpoint
    reserved = _reserved_models()
    deadline = time.monotonic() + RESOLVER_BUDGET_SECS

    # Los providers puenteados comparten endpoint: se consulta una sola vez.
    probes: dict[str, list[Any] | None] = {}
    raw_updates: dict[str, dict[str, Any]] = {}
    warnings: list[str] = []

    for definition in PROVIDER_REGISTRY.values():
        owner = getattr(definition, "bridge_model_owner", None)
        if not owner:
            continue
        budget_left = deadline - time.monotonic()
        if budget_left <= 0:
            warnings.append("resolver: presupuesto global agotado")
            break
        try:
            resolved = resolve_fn(definition.provider_id, env)
        except Exception:  # noqa: BLE001 — una credencial rota no frena el resto
            continue
        if not isinstance(resolved, dict) or not resolved.get("ok"):
            continue
        base_url = resolved.get("base_url")
        api_key = resolved.get("api_key")
        if not isinstance(base_url, str) or not base_url:
            continue
        if not isinstance(api_key, str) or not api_key:
            continue
        if base_url not in probes:
            probes[base_url] = probe_fn(base_url, api_key, deadline - time.monotonic())
        models = models_for_owner(probes[base_url], owner, reserved)
        if not models:
            # Un provider usable sin modelos seleccionables sería una mentira.
            continue
        raw_updates[definition.provider_id] = {
            "usable_now": True,
            "status": "usable",
            "availability": "RESOLVER_PROBED",
            "models_status": "probed_live",
            "bridge_status": "bridge_running",
            "credential_detected": True,
            "base_url_detected": base_url,
            "next_action": None,
            "models": models,
        }

    deduped, collision_warnings = apply_precedence(raw_updates)
    warnings.extend(collision_warnings)

    updates: dict[str, dict[str, Any]] = {}
    for provider_id, update in deduped.items():
        models = update.get("models", [])
        if not models:
            continue
        if len(models) > SECTION_MODEL_WARN_THRESHOLD:
            warnings.append(
                f"resolver: '{provider_id}' expone {len(models)} modelos "
                f"(> {SECTION_MODEL_WARN_THRESHOLD}); no se truncó"
            )
        update["models_detected"] = list(models)
        update["model_policy"] = {
            "user_facing_models": list(models),
            "user_facing_count": len(models),
            "reserved_internal_models": [],
            "reserved_internal_count": 0,
            "all_models_reserved": False,
            "selectable_models": list(models),
        }
        updates[provider_id] = update

    if not updates:
        return SourceResult.failed(
            "; ".join(warnings) if warnings else "sin providers promovibles"
        )
    return SourceResult(True, updates, "; ".join(warnings) if warnings else None)


def _direct_key_source(
    paths: ReconcilePaths,
    env: Mapping[str, str],
    *,
    cache: VerificationCache | None = None,
) -> SourceResult:
    """Providers con credencial propia en disco (familia OpenCode, MiMo, …).

    Los almacenes salen del registry, en su orden de precedencia. Se publica
    QUÉ almacén ganó: en esta máquina conviven cuatro valores distintos de la
    key de OpenCode y saber cuál se usó es la diferencia entre diagnosticar y
    adivinar.

    Un provider sólo llega a `usable_now` si su credencial fue **verificada**.
    Una credencial válida sin saldo se publica como detectada y NO usable, que
    es la verdad: la key sirve, la cuenta no tiene crédito.
    """
    if not flag_enabled(env):
        return SourceResult.failed(RESOLVER_DISABLED_REASON)

    store = cache if cache is not None else VerificationCache()
    deadline = time.monotonic() + RESOLVER_BUDGET_SECS
    reserved = _reserved_models()
    updates: dict[str, dict[str, Any]] = {}
    warnings: list[str] = []

    for definition in PROVIDER_REGISTRY.values():
        if not getattr(definition, "credential_stores", ()):
            continue
        if time.monotonic() >= deadline:
            warnings.append("direct_key: presupuesto global agotado")
            break
        try:
            resolved = resolve_from_stores(definition, env)
        except Exception:  # noqa: BLE001 — un almacén roto no frena a los demás
            continue
        if resolved is None:
            continue

        base_url = (resolved.base_url or definition.base_url_hint or "").rstrip("/")
        models = models_for_owner(
            _probe_models_endpoint(
                base_url, resolved.secret, deadline - time.monotonic()
            )
            if base_url
            else None,
            None,
            reserved,
        )
        # Tier libre: los modelos marcados responden SIN credencial. Se los
        # separa porque un provider puede tener la credencial sin saldo y aun
        # así servir estos: negarlos sería tan falso como prometer los de pago.
        # Partición por pricing DECLARADO: un provider puede servir modelos
        # gratis SIN credencial y de pago CON ella, en el mismo endpoint. El
        # marcador por sufijo quedó atrás — perdía `big-pickle`, que es gratis
        # y no termina en "-free".
        split = model_availability.split_models(definition, models, env)
        free_models = list(split.free)
        if not split.determined and getattr(definition, "model_catalog_sources", ()):
            # Hay fuentes declaradas y ninguna resolvió: no se adivina.
            # Mostrar todos como usables sería afirmar sin verificar; mostrar
            # ninguno sería esconder el provider.
            updates[definition.provider_id] = {
                "credential_state": model_availability.UNKNOWN_AVAILABILITY,
                "credential_detail": model_availability.UNKNOWN_AVAILABILITY_DETAIL,
                "usable_now": False,
                "models": list(models),
                "models_detected": list(models),
                "models_status": model_availability.UNKNOWN_AVAILABILITY,
                "status": "detected_login",
                "availability": "CREDENTIAL_PRESENT",
                "credential_detected": True,
                "next_action": "connect",
                **resolved.safe_summary(),
            }
            continue

        probe_model = models[0] if models else None
        verdict = credential_verifier.verify_credential(
            definition, resolved, cache=store, probe_model=probe_model
        )

        if free_models and not verdict.usable:
            # UN canario, no siete: probar cada modelo del conjunto gastaría
            # N llamadas para demostrar lo mismo.
            canary = model_availability.free_canary(split) or free_models[0]
            free_verdict = credential_verifier.verify_free_tier(
                definition.verify_endpoint or "", canary
            )
            if free_verdict.usable:
                # El tier libre existe y está verificado, pero el runtime NO
                # puede consumirlo todavía: `LlmProvider::from_config`
                # (nexum-acp/src/provider/mod.rs:149) descarta cualquier
                # provider con api_key vacía, y si se le pasa un placeholder el
                # proveedor responde 401 «Invalid API key» — el tier libre
                # exige NO mandar Authorization.
                #
                # Marcarlo usable sería mentir en la UI. Se publica el estado
                # real, con los modelos y el motivo del bloqueo.
                verdict = free_verdict
                models = free_models

        update: dict[str, Any] = {
            "credential_detected": True,
            "native_login_detected": True,
            "credential_state": verdict.state,
            "credential_detail": verdict.detail,
            "credential_verification_cached": verdict.cached,
            "base_url_detected": base_url or None,
            # `free_access` YA es usable: el runtime aprendió a hablar sin
            # credencial (nexum-acp, disparado por este mismo estado del
            # catálogo). Antes se excluía a propósito para no prometer en la
            # UI algo que el chat no podía cumplir.
            "usable_now": bool(verdict.usable and models),
            **resolved.safe_summary(),
        }
        if resolved.store_legacy:
            warnings.append(
                f"{definition.provider_id}: credencial tomada de un almacén legacy "
                f"({resolved.store_path})"
            )
        if verdict.usable and models:
            update.update(
                {
                    "status": "usable",
                    "availability": "CREDENTIAL_VERIFIED",
                    "models_status": "probed_live",
                    "models": models,
                    "models_detected": list(models),
                    "next_action": None,
                    "model_policy": {
                        "user_facing_models": list(models),
                        "user_facing_count": len(models),
                        "reserved_internal_models": [],
                        "reserved_internal_count": 0,
                        "all_models_reserved": False,
                        "selectable_models": list(models),
                    },
                }
            )
        else:
            update.update(
                {
                    "status": "detected_login",
                    "availability": "CREDENTIAL_PRESENT",
                    "models_status": verdict.state,
                    "status_detail": verdict.detail,
                    "next_action": "connect",
                }
            )
        updates[definition.provider_id] = update

    if not updates:
        return SourceResult.failed(
            "; ".join(warnings) if warnings else "sin credenciales directas en disco"
        )
    return SourceResult(True, updates, "; ".join(warnings) if warnings else None)


# Cómo se conecta cada provider, DERIVADO de su auth_mode. El panel enruta por
# este campo en vez de por una lista de ids escrita a mano: antes
# `BRIDGE_PROVIDERS` tenía 3 ids fijos y para el resto el Enter no hacía nada,
# que es peor que fallar porque no se distingue de un cuelgue.
CONNECT_KIND_BY_AUTH_MODE = {
    "cli_oauth": "bridge_oauth",
    "direct_key": "credential_store",
    "static_api_key": "api_key_input",
    "openai_compatible": "api_key_input",
    "bridge_proxy": "bridge_oauth",
    "local_no_auth": None,
}

# Comando con el que el usuario vuelve a loguearse en la CLI dueña de la
# credencial. Declarativo: el panel lo muestra, no lo ejecuta por su cuenta.
RELOGIN_COMMAND_BY_FAMILY = {
    "opencode_zen": "opencode auth login",
    "opencode_go": "opencode auth login",
    "mimo_code": "mimo auth login",
}



# ─── Canario de ejecución por modelo ─────────────────────────────────────────

# Presupuesto por corrida. Con 65 modelos declarados, un canario sin tope serían
# 65 requests en cada reconcile; así la cobertura se completa en varias corridas
# en vez de golpear de una. Ver src/nexum_providers/model_canary.py.
CANARY_PROBES_POR_CORRIDA = 3
CANARY_FLAG = "NEXUM_MODEL_CANARY"


def annotate_model_canary(catalog: dict[str, Any]) -> dict[str, Any]:
    """Marca los modelos que el proveedor NO reconoce.

    El catálogo declaraba modelos usables que devolvían 404 al chatear, y nadie
    lo detectaba: Doctor valida forma y el registry gate valida correspondencia,
    pero ninguno toca al proveedor. Esto cierra ese borde.

    Sólo agrega información: un modelo con veredicto `not_found` queda listado
    en `models_unavailable`, y el resto del catálogo no cambia. Un canario que
    borrara modelos podría dejar al usuario sin ninguno por un error transitorio.
    """
    # Default OFF: el canario hace requests reales, y encender algo que gasta
    # cuota sin que nadie lo pida es lo contrario de lo que venimos haciendo.
    if os.environ.get(CANARY_FLAG, "").lower() not in _FLAG_TRUTHY:
        return catalog
    try:
        from nexum_providers.model_canary import NOT_FOUND, probe_models
    except ImportError:
        return catalog

    # La credencial NO está en el catálogo (se redacta a propósito), así que se
    # resuelve por el mismo camino que usa la verificación de credencial. Sin
    # key el probe da 401, que el canario clasifica UNKNOWN y no NOT_FOUND — no
    # condena al modelo, pero tampoco sirve de nada.
    from nexum_providers import provider_resolve

    env = dict(os.environ)
    candidatos: list[tuple[str, str, str, str | None]] = []
    for entry in catalog.get("providers", []):
        # Sólo providers usables: probar el modelo de uno sin credencial no dice
        # nada del modelo.
        if not entry.get("usable_now"):
            continue
        pid = entry.get("provider_id") or entry.get("id")
        try:
            resuelto = provider_resolve.resolve(pid, env=env)
        except Exception:  # noqa: BLE001
            continue
        if not isinstance(resuelto, dict) or not resuelto.get("ok"):
            continue
        url, key = resuelto.get("base_url"), resuelto.get("api_key")
        if not isinstance(url, str) or not url:
            continue
        for model in entry.get("models") or []:
            candidatos.append(
                (pid, model, f"{url.rstrip('/')}/chat/completions", key or None)
            )

    try:
        veredictos = probe_models(candidatos, max_probes=CANARY_PROBES_POR_CORRIDA)
    except Exception as exc:  # noqa: BLE001 — un canario no puede tirar el reconcile
        catalog.setdefault("notes", []).append(f"canario de modelos omitido: {exc}")
        return catalog

    for entry in catalog.get("providers", []):
        pid = entry.get("provider_id") or entry.get("id")
        malos = [
            m
            for m in entry.get("models") or []
            if (v := veredictos.get(f"{pid}::{m}")) is not None
            and v.verdict == NOT_FOUND
        ]
        if malos:
            entry["models_unavailable"] = malos
    return catalog


def annotate_connect_affordances(catalog: dict[str, Any]) -> dict[str, Any]:
    """Publica `connect_kind` y `relogin_command` para CADA provider del catálogo.

    Sin esto, la TUI no tiene forma de saber qué acción ofrecer salvo mirando
    ids concretos, que es justo lo que produjo el no-op silencioso de RC-7.
    """
    for provider in catalog.get("providers", []):
        if not isinstance(provider, dict):
            continue
        provider_id = _provider_id(provider)
        definition = PROVIDER_REGISTRY.get(provider_id)
        auth_mode = provider.get("auth_mode") or (
            definition.auth_mode if definition else None
        )
        provider["connect_kind"] = CONNECT_KIND_BY_AUTH_MODE.get(auth_mode or "", None)
        command = RELOGIN_COMMAND_BY_FAMILY.get(provider_id)
        if command:
            provider["relogin_command"] = command
    return catalog


def _cache_intermediates(
    paths: ReconcilePaths, sources: Mapping[str, SourceResult]
) -> None:
    paths.cache_dir.mkdir(parents=True, exist_ok=True)
    doctor = {
        "schema_version": 1,
        "generated_at": _now(),
        "sources": {name: result.available for name, result in sources.items()},
    }
    resolver = {
        "schema_version": 1,
        "generated_at": _now(),
        "providers": [
            {"provider_id": pid, "usable_now": update.get("usable_now", False)}
            for result in sources.values()
            for pid, update in result.updates.items()
        ],
    }
    _atomic_write(paths.cache_dir / "provider-doctor-output.json", doctor)
    _atomic_write(paths.cache_dir / "provider-resolver-output.json", resolver)


def reconcile(
    paths: ReconcilePaths, base_catalog_path: Path, env: Mapping[str, str] | None = None
) -> dict[str, Any]:
    """Run all R2 safe sources and atomically publish a live catalog."""
    environment = dict(os.environ if env is None else env)
    base = load_valid_catalog(base_catalog_path)
    sources = {
        SOURCE_NATIVE_CLIS: _native_cli_source(),
        SOURCE_NATIVE_LOGINS: _native_login_source(),
        SOURCE_BRIDGE_CLIPROXYAPI: _bridge_source(),
        SOURCE_DIRECT_PROVIDER_RESOLVER: _resolver_source(paths, environment),
        SOURCE_DIRECT_KEY_STORES: _direct_key_source(paths, environment),
        SOURCE_LOCAL_OLLAMA: _ollama_source(),
        SOURCE_KEYSTORE: _keystore_source(paths),
        SOURCE_ENV_AUTOLOGIN: _env_source(environment),
        SOURCE_PREVIOUS_VALID_SNAPSHOT: SourceResult.ok({})
        if load_valid_catalog(paths.previous_catalog) is not None
        else SourceResult.failed("no previous valid snapshot"),
    }
    _cache_intermediates(paths, sources)
    catalog = annotate_connect_affordances(build_catalog(base, sources))
    catalog = annotate_model_canary(catalog)
    catalog["catalog_generation"] = CATALOG_GENERATION
    if flag_enabled(environment):
        # Marcador persistente: permite verificar desde el catálogo publicado
        # que el binario en ejecución es el que lleva este cambio.
        catalog["notes"].append(LOG_MARKER_PROVIDER_CONNECT_V2)
    publish_catalog(paths, catalog)
    _atomic_write(
        paths.status,
        {
            "schema_version": 1,
            "generated_at": _now(),
            "ok": True,
            "catalog_kind": catalog["catalog_kind"],
            "provider_count": len(catalog["providers"]),
            "warnings": catalog["generation_warnings"],
        },
    )
    return catalog


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Nexum installed provider reconcile")
    parser.add_argument("--base", required=True, help="Installed base catalog path")
    parser.add_argument("--quiet", action="store_true", help="Suppress safe summary")
    args = parser.parse_args(argv)
    paths = ReconcilePaths.from_environment()
    try:
        catalog = reconcile(paths, Path(args.base))
    except (OSError, ValueError) as exc:
        # Errors intentionally include no paths or source payloads: this command
        # can be launched by the installed runtime on a user's machine.
        print(f"provider reconcile failed: {type(exc).__name__}")
        return 1
    if not args.quiet:
        usable = sum(
            1 for provider in catalog["providers"] if provider.get("usable_now")
        )
        print(
            f"provider reconcile: {catalog['catalog_kind']}; "
            f"{len(catalog['providers'])} supported; {usable} usable"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
