#!/usr/bin/env python3
"""Nexum Provider Catalog v2 — consolidated provider catalog (ADR-044 Capa 3).

Orchestrates the detection layers into a single catalog JSON consumed by the
`/proveedor` and `/modelo` TUI panels:

    Registry (Capa 0) → native detection (Capa 1) → bridge/OpenCode (Capa 2)
                                                        ↓
                                              provider-catalog-output.json v2

Output schema (v2, additive over v1 — the Rust panel uses #[serde(default)] so
v1 fields + v2 fields coexist):

    {
      "version": "2",
      "catalog_version": "2",         # backward-compat alias
      "generated_at": ...,
      "cli_proxy_api": {installed, running, port, status, detail},
      "recommended_provider_id": "ollama_local",
      "active_provider_id": "ollama-local" | null,
      "reserved_models": [...],
      "providers": [{
        # v2 fields (ADR-044):
        "provider_id", "family", "auth_mode",
        "native_login_detected", "bridge_status", "bridge_detail",
        "direct_key_status", "credential_path_safe",
        "email", "last_refresh", "models", "models_status",
        "next_action", "usable_now",
        # v1 backward-compat fields (Rust panel reads these):
        "id", "display_name", "category", "connection_type",
        "status", "status_detail", "recommended", "credential_detected",
        "credential_fingerprint", "base_url_detected", "description",
        "models_detected", "model_policy"
      }],
      "notes": [...]
    }

Security: NO tokens, API keys, refresh tokens, or access tokens in the output.
Keys are masked to first2…last4. Native login detection records only existence,
path, mtime, size, type — never token values.

Stdlib only. No Nexum runtime imports.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

# Productive package (src/nexum_providers). Running this file directly remains
# supported for the historic CLI, while package imports stay relative.
_HERE = Path(__file__).resolve().parent  # .../catalog_gen/
_SRC_DIR = _HERE.parent.parent  # .../src/
if __package__ in (None, "") and str(_SRC_DIR) not in sys.path:
    sys.path.insert(0, str(_SRC_DIR))

# E402: estos imports requieren el sys.path bootstrap de arriba (src/ + este dir).
from nexum_providers.catalog_providers import (  # noqa: E402
    CATALOG_PROVIDERS,
    get_catalog_entry,
)
from nexum_providers.key_store import KeyStore  # noqa: E402
from nexum_providers.probe_validator import probe_api_key  # noqa: E402
from nexum_providers.provider_login import build_usable_entry  # noqa: E402

# Same-package imports (detection layers).
from .provider_registry import (  # noqa: E402
    PROVIDER_REGISTRY,
    AuthMode,
    DetectionStatus,
    ProviderDefinition,
)
from .native_login_detector import NativeLoginInfo, detect_native_login  # noqa: E402
from .cliproxyapi_bridge import (  # noqa: E402
    BridgeStatus,
    CLIProxyAPIBridge,
    auth_file_for,
)
from .opencode_family_detector import (  # noqa: E402
    OpenCodeFamilyResult,
    detect_opencode_family,
)
from .mimo_detector import MiMoResult, detect_mimo  # noqa: E402
from .reserved_models_lib import reserved_entries, reserved_model_names  # noqa: E402

VERSION = "2"
DEFAULT_DOCTOR = "provider-doctor-output.json"
DEFAULT_RESOLVER = "provider-resolver-output.json"
DEFAULT_OUTPUT = "provider-catalog-output.json"

# v1 status constants kept for backward-compat mapping.
STATUS_USABLE_NOW = "usable_now"
STATUS_CONNECTED = "connected"
STATUS_DETECTED_LOGIN = "detected_login"
STATUS_REQUIRES_API_KEY = "requires_api_key"
STATUS_REQUIRES_ADAPTER = "requires_adapter"
STATUS_NOT_CONFIGURED = "not_configured"


# ─── Helpers ──────────────────────────────────────────────────────────────────


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _load_json(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise FileNotFoundError(f"input not found: {path}")
    with path.open("r", encoding="utf-8") as fh:
        return json.load(fh)


def _doctor_lookup(doctor: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """Flatten doctor entries by lowercased name across all sections."""
    out: dict[str, dict[str, Any]] = {}
    for section in ("api_keys", "oauth_logins", "local_servers", "clis"):
        for entry in doctor.get(section, []) or []:
            n = entry.get("name")
            if isinstance(n, str):
                out[n.lower()] = entry
    return out


def _resolver_map(resolver: dict[str, Any]) -> dict[str, dict[str, Any]]:
    out: dict[str, dict[str, Any]] = {}
    for p in resolver.get("providers", []) or []:
        for key in (p.get("source"), p.get("name")):
            if isinstance(key, str):
                out[key.lower()] = p
    return out


def _mask(value: str) -> str:
    v = str(value).strip()
    if len(v) <= 6:
        return "<redacted>"
    return f"{v[:2]}…{v[-4:]}"


def _detect_active_provider_id(cli_root: Path) -> str | None:
    settings = cli_root / ".peri" / "settings.json"
    if not settings.is_file():
        return None
    try:
        data = json.loads(settings.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    cfg = data.get("config") if isinstance(data, dict) else None
    if isinstance(cfg, dict):
        ap = cfg.get("active_provider_id")
        if isinstance(ap, str):
            return ap
    return None


def _detect_ollama_models(resolver_map: dict[str, dict[str, Any]]) -> list[str]:
    """Get Ollama's real models from the resolver output (probed earlier)."""
    r = resolver_map.get("ollama") or {}
    return list(r.get("models", []) or [])


# ─── Per-detector classification ──────────────────────────────────────────────


# Prefix heuristic to attribute the bridge's /v1/models list to a provider
# when `owned_by` is absent or ambiguous.
_BRIDGE_MODEL_PREFIXES: dict[str, tuple[str, ...]] = {
    "anthropic": ("claude",),
    "codex": ("gpt", "codex", "o1", "o3", "o4"),
    "antigravity": ("gemini",),
}


def _bridge_models_for(
    definition: ProviderDefinition, live_models: list[dict[str, Any]] | None
) -> list[str]:
    """Attribute CLIProxyAPI /v1/models entries to this provider (best effort)."""
    if not live_models:
        return []
    cpid = definition.cliproxy_provider_id or ""
    prefixes = _BRIDGE_MODEL_PREFIXES.get(cpid, ())
    out: list[str] = []
    for m in live_models:
        mid = str(m.get("id") or "")
        if not mid:
            continue
        owned = str(m.get("owned_by") or "").lower()
        if cpid and owned and cpid in owned:
            out.append(mid)
        elif any(mid.lower().startswith(p) for p in prefixes):
            out.append(mid)
    return out


def _classify_cli_oauth(
    definition: ProviderDefinition,
    native: NativeLoginInfo,
    bridge: BridgeStatus,
    live_models: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    """Classify a cli_oauth provider (Claude/Codex/Gemini) via native + bridge."""
    pid = definition.provider_id
    family = definition.family

    if not native.detected:
        return {
            "provider_id": pid,
            "family": family,
            "auth_mode": AuthMode.CLI_OAUTH,
            "native_login_detected": False,
            "bridge_status": "native_not_found",
            "bridge_detail": "No se encontró sesión nativa en disco.",
            "usable_now": False,
            "models": [],
            "models_status": "no_login",
            "next_action": "login_native_cli",
            "status": DetectionStatus.NOT_INSTALLED,
            "status_detail": "No se encontró sesión nativa en disco.",
            "credential_detected": False,
            "credential_path_safe": None,
        }

    # Native login present — now depends on bridge.
    if bridge.status == DetectionStatus.BRIDGE_NOT_INSTALLED:
        return {
            "provider_id": pid,
            "family": family,
            "auth_mode": AuthMode.CLI_OAUTH,
            "native_login_detected": True,
            "bridge_status": bridge.status,
            "bridge_detail": bridge.detail,
            "usable_now": False,
            "models": [],
            "models_status": "bridge_required",
            "next_action": "instalar_cliproxyapi",
            "status": DetectionStatus.BRIDGE_NOT_INSTALLED,
            "status_detail": (
                f"Login nativo de {family} detectado, pero CLIProxyAPI no está "
                "instalado. Instalalo para activar el puente."
            ),
            "credential_detected": True,
            "credential_path_safe": native.credential_path,
        }

    if bridge.status == DetectionStatus.BRIDGE_NOT_RUNNING:
        return {
            "provider_id": pid,
            "family": family,
            "auth_mode": AuthMode.CLI_OAUTH,
            "native_login_detected": True,
            "bridge_status": bridge.status,
            "bridge_detail": bridge.detail,
            "usable_now": False,
            "models": [],
            "models_status": "bridge_required",
            "next_action": "iniciar_cliproxyapi",
            "status": DetectionStatus.BRIDGE_NOT_RUNNING,
            "status_detail": (
                f"Login nativo de {family} detectado. CLIProxyAPI instalado pero "
                "no corriendo. Iniciarlo: systemctl --user start cli-proxy-api"
            ),
            "credential_detected": True,
            "credential_path_safe": native.credential_path,
        }

    if bridge.status == DetectionStatus.BRIDGE_MANAGEMENT_LOCKED:
        return {
            "provider_id": pid,
            "family": family,
            "auth_mode": AuthMode.CLI_OAUTH,
            "native_login_detected": True,
            "bridge_status": bridge.status,
            "bridge_detail": bridge.detail,
            "usable_now": False,
            "models": [],
            "models_status": "management_locked",
            "next_action": "configurar_management_key",
            "status": DetectionStatus.BRIDGE_MANAGEMENT_LOCKED,
            "status_detail": bridge.detail,
            "credential_detected": True,
            "credential_path_safe": native.credential_path,
        }

    # bridge_ok — check if THIS provider is bridged.
    auth = auth_file_for(bridge.auth_files, definition.cliproxy_provider_id or "")
    if auth is None:
        return {
            "provider_id": pid,
            "family": family,
            "auth_mode": AuthMode.CLI_OAUTH,
            "native_login_detected": True,
            "bridge_status": "bridge_not_active",
            "bridge_detail": (
                f"Login nativo detectado, pero el puente no fue activado para "
                f"{family}. Acción: conectar puente."
            ),
            "usable_now": False,
            "models": [],
            "models_status": "bridge_required",
            "next_action": "conectar_puente",
            "status": DetectionStatus.BRIDGE_NOT_ACTIVE,
            "status_detail": (
                f"Login nativo de {family} detectado. Activar el puente desde "
                "/proveedor para usarlo."
            ),
            "credential_detected": True,
            "credential_path_safe": native.credential_path,
            "email": None,
            "last_refresh": None,
        }

    # Bridged — check status of the auth entry.
    auth_status = str(auth.get("status", "")).lower()
    disabled = bool(auth.get("disabled") or auth.get("unavailable"))
    status_message = str(auth.get("status_message") or "")

    # Límite de uso / rate limit del plan (E2E 2026-07-06: Codex quedó
    # unavailable con status_message usage_limit_reached tras 429s). Es un
    # estado TEMPORAL del upstream: re-loguear no lo arregla, así que NO
    # mandamos al usuario a "reconectar puente" — la cuenta se recupera sola.
    rate_limited = disabled and any(
        marker in status_message.lower()
        for marker in ("usage_limit", "rate_limit", "too many requests", "429")
    )
    if rate_limited:
        models = _bridge_models_for(definition, live_models) or [
            m.model_id for m in definition.static_models
        ]
        return {
            "provider_id": pid,
            "family": family,
            "auth_mode": AuthMode.CLI_OAUTH,
            "native_login_detected": True,
            "bridge_status": "rate_limited",
            "bridge_detail": (
                f"{family} conectado, pero el plan alcanzó su límite de uso. "
                "Se recupera solo — no hace falta re-loguear."
            ),
            "usable_now": False,
            "models": models,
            "models_status": "rate_limited",
            "next_action": None,
            "status": "rate_limited",
            "status_detail": (
                f"{family} puenteado y autenticado; límite de uso del plan "
                "alcanzado. Reintentá más tarde."
            ),
            "credential_detected": True,
            "credential_path_safe": native.credential_path,
            "email": auth.get("email"),
            "last_refresh": auth.get("last_refresh"),
        }

    if disabled:
        return {
            "provider_id": pid,
            "family": family,
            "auth_mode": AuthMode.CLI_OAUTH,
            "native_login_detected": True,
            "bridge_status": "error",
            "bridge_detail": str(
                auth.get("status_message", "Cuenta deshabilitada o no disponible.")
            ),
            "usable_now": False,
            "models": [],
            "models_status": "bridge_disabled",
            "next_action": "reconectar_puente",
            "status": DetectionStatus.ERROR,
            "status_detail": str(
                auth.get("status_message", "Cuenta puente deshabilitada.")
            ),
            "credential_detected": True,
            "credential_path_safe": native.credential_path,
            "email": auth.get("email"),
            "last_refresh": auth.get("last_refresh"),
        }

    # 7.2.50 real reporta "active" en auth-files (verificado E2E 2026-07-06);
    # "ready" era el valor asumido antes de ver la respuesta real. Aceptamos
    # ambos: cualquiera de los dos significa token vigente y cuenta puenteada.
    if auth_status in ("ready", "active"):
        models = _bridge_models_for(definition, live_models) or [
            m.model_id for m in definition.static_models
        ]
        return {
            "provider_id": pid,
            "family": family,
            "auth_mode": AuthMode.CLI_OAUTH,
            "native_login_detected": True,
            "bridge_status": "usable",
            "bridge_detail": "Puenteado vía CLIProxyAPI, cuenta activa.",
            "usable_now": True,
            "models": models,
            "models_status": "static" if models else "probe_pending",
            "next_action": None,
            "status": DetectionStatus.USABLE,
            "status_detail": f"{family} puenteado y activo vía CLIProxyAPI.",
            "credential_detected": True,
            "credential_path_safe": native.credential_path,
            "email": auth.get("email"),
            "last_refresh": auth.get("last_refresh"),
        }

    # Non-ready status → expired/needs refresh.
    return {
        "provider_id": pid,
        "family": family,
        "auth_mode": AuthMode.CLI_OAUTH,
        "native_login_detected": True,
        "bridge_status": "expired",
        "bridge_detail": str(
            auth.get("status_message", "El token del puente venció o falló.")
        ),
        "usable_now": False,
        "models": [],
        "models_status": "bridge_expired",
        "next_action": "reconectar_puente",
        "status": DetectionStatus.EXPIRED,
        "status_detail": "Token del puente venció. Reconectar desde /proveedor.",
        "credential_detected": True,
        "credential_path_safe": native.credential_path,
        "email": auth.get("email"),
        "last_refresh": auth.get("last_refresh"),
    }


def _classify_opencode_family(result: OpenCodeFamilyResult) -> dict[str, Any]:
    """Map an OpenCodeFamilyResult to a catalog provider dict."""
    # v1-compat status mapping.
    v1_status = {
        DetectionStatus.USABLE: STATUS_USABLE_NOW,
        DetectionStatus.PROBE_PENDING: STATUS_CONNECTED,
        DetectionStatus.PROBE_FAILED: STATUS_DETECTED_LOGIN,
        DetectionStatus.NOT_INSTALLED: STATUS_NOT_CONFIGURED,
        DetectionStatus.ERROR: DetectionStatus.ERROR,
    }.get(result.status, result.status)

    return {
        "provider_id": result.provider_id,
        "family": result.family,
        "auth_mode": AuthMode.DIRECT_KEY,
        "native_login_detected": result.credential_detected,
        "bridge_status": "direct_key" if result.credential_detected else "no_key",
        "bridge_detail": result.detail,
        "usable_now": result.usable_now,
        "models": result.models,
        "models_status": result.models_status,
        "next_action": result.next_action,
        "status": result.status if result.usable_now else v1_status,
        "status_detail": result.detail,
        "credential_detected": result.credential_detected,
        "credential_path_safe": result.auth_json_path,
        "credential_fingerprint": result.credential_fingerprint,
        "base_url_detected": result.base_url,
        "email": None,
        "last_refresh": None,
    }


def _classify_mimo(result: MiMoResult) -> dict[str, Any]:
    return {
        "provider_id": result.provider_id,
        "family": result.family,
        "auth_mode": AuthMode.DIRECT_KEY,
        "native_login_detected": True,
        "bridge_status": "mimo_different_format",
        "bridge_detail": result.detail,
        "usable_now": False,
        "models": [],
        "models_status": "mimo_adapter_required",
        "next_action": result.next_action,
        "status": result.status,
        "status_detail": result.detail,
        "credential_detected": True,
        "credential_path_safe": result.data_dir or result.config_path,
        "credential_fingerprint": None,
        "base_url_detected": None,
        "email": None,
        "last_refresh": None,
    }


def _classify_ollama(
    definition: ProviderDefinition,
    resolver_map: dict[str, dict[str, Any]],
    doctor_map: dict[str, dict[str, Any]],
    reserved: set[str],
) -> dict[str, Any]:
    """Ollama stays conserved — reads resolver for usable_now + models."""
    r = resolver_map.get("ollama") or {}
    usable = bool(r.get("usable_now"))
    models_all = list(r.get("models", []) or [])
    # Apply reserved-model policy.
    user_facing = [m for m in models_all if m not in reserved]
    reserved_here = [m for m in models_all if m in reserved]
    all_reserved = bool(models_all) and not user_facing

    model_policy: dict[str, Any] = {}
    if models_all:
        model_policy = {
            "reserved_internal_models": reserved_here,
            "reserved_internal_count": len(reserved_here),
            "user_facing_models": user_facing,
            "user_facing_count": len(user_facing),
            "all_models_reserved": all_reserved,
            "policy": "internal_only models are filtered from /modelo; "
            "Hormiguero reads them directly from Ollama.",
            "selectable_models": user_facing,
        }

    status = (
        DetectionStatus.USABLE
        if usable and not all_reserved
        else ("all_models_reserved" if all_reserved else DetectionStatus.NOT_INSTALLED)
    )
    detail = (
        r.get("reason") or "Local Ollama."
        if usable
        else (
            "Ollama responde pero todos los modelos están reservados para el Hormiguero. "
            "No hay modelo user-facing disponible."
            if all_reserved
            else "Ollama no está corriendo."
        )
    )

    return {
        "provider_id": definition.provider_id,
        "family": definition.family,
        "auth_mode": AuthMode.LOCAL_NO_AUTH,
        "native_login_detected": usable,
        "bridge_status": "local_no_auth",
        "bridge_detail": detail,
        "usable_now": usable and not all_reserved,
        "models": user_facing,  # user-facing only in top-level models[]
        "models_status": "ok" if usable else "local_server_down",
        "next_action": None if usable else "iniciar_ollama",
        "status": status,
        "status_detail": detail,
        "credential_detected": False,
        "credential_path_safe": None,
        "base_url_detected": "http://127.0.0.1:11434/v1" if usable else None,
        "email": None,
        "last_refresh": None,
        "model_policy": model_policy,
    }


def _classify_static_api_key(
    definition: ProviderDefinition,
    env: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Static API-key provider — detect only explicit process environment.

    This lets a freshly-installed user export ZAI_CODING_API_KEY and see
    glm_coding_plan surface as detected in /provedor, ready for auto-login.

    The env var presence only flags the provider as detected+probe_pending;
    it never marks it usable_now (that requires an actual probe / auto-login).
    """
    cred_detected = False
    fingerprint: str | None = None
    source: str | None = None

    live_env = env if env is not None else dict(os.environ)
    for ev in definition.env_vars:
        val = live_env.get(ev, "").strip()
        if val:
            cred_detected = True
            source = f"env:{ev}"
            fingerprint = _mask(val)
            break

    if cred_detected:
        return {
            "provider_id": definition.provider_id,
            "family": definition.family,
            "auth_mode": definition.auth_mode,
            "native_login_detected": True,
            "bridge_status": "direct_key",
            "bridge_detail": f"API key detectada en {source}.",
            "usable_now": False,  # needs explicit probe to confirm
            "models": [],
            "models_status": "probe_pending",
            "next_action": "auto_login_available",
            "status": STATUS_CONNECTED,
            "status_detail": f"API key detectada en {source}. Auto-login disponible.",
            "credential_detected": True,
            "credential_path_safe": source,
            "credential_fingerprint": fingerprint,
            "base_url_detected": definition.base_url_hint,
            "email": None,
            "last_refresh": None,
        }

    return {
        "provider_id": definition.provider_id,
        "family": definition.family,
        "auth_mode": definition.auth_mode,
        "native_login_detected": False,
        "bridge_status": "no_key",
        "bridge_detail": "Requiere API key manual.",
        "usable_now": False,
        "models": [],
        "models_status": "no_key",
        "next_action": "ingresar_api_key",
        "status": STATUS_REQUIRES_API_KEY,
        "status_detail": f"{definition.family} requiere una API key para conectar.",
        "credential_detected": False,
        "credential_path_safe": None,
        "base_url_detected": definition.base_url_hint,
        "email": None,
        "last_refresh": None,
    }


def _classify_openai_compatible(
    definition: ProviderDefinition,
    env: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Require both a custom endpoint and API key before declaring direct-key auth."""
    env_map = env if env is not None else dict(os.environ)
    api_key = env_map.get("OPENAI_API_KEY", "").strip()
    base_url = (
        env_map.get("OPENAI_BASE_URL", "").strip()
        or env_map.get("OPENAI_API_BASE", "").strip()
    )
    if api_key and base_url:
        entry = _classify_static_api_key(definition, {"OPENAI_API_KEY": api_key})
        entry["base_url_detected"] = base_url.rstrip("/")
        return entry
    entry = _classify_static_api_key(definition, {})
    entry["bridge_detail"] = "Requiere API key y base URL compatibles con OpenAI."
    entry["next_action"] = "ingresar_api_key_y_base_url"
    return entry


# ─── Orchestrator ─────────────────────────────────────────────────────────────


def build_catalog(
    doctor_path: Path,
    resolver_path: Path,
    *,
    probe_online: bool = False,
    bridge: CLIProxyAPIBridge | None = None,
    key_store: KeyStore | None = None,
    env: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Build the v2 catalog. Reads doctor/resolver JSON, runs all detectors.

    `probe_online=True` enables live /models probes for OpenCode-family + static
    API-key providers (uses their keys against provider endpoints). Default False
    = no network calls beyond localhost bridge/Ollama.
    `bridge` can be injected for testing.
    """
    doctor = _load_json(doctor_path)
    resolver = _load_json(resolver_path)
    environment = dict(os.environ if env is None else env)
    resolver_map = _resolver_map(resolver)
    doctor_map = _doctor_lookup(doctor)
    reserved = reserved_model_names()
    cli_root = doctor_path.parent

    # Bridge status (computed once).
    if bridge is None:
        bridge = CLIProxyAPIBridge()
    bridge_status = bridge.status()
    # getattr: bridges inyectados en tests pueden no implementar list_models.
    live_bridge_models = (
        getattr(bridge, "list_models", lambda: [])()
        if bridge_status.status == "bridge_ok"
        else []
    )

    providers: list[dict[str, Any]] = []
    notes: list[str] = []

    for pid, definition in PROVIDER_REGISTRY.items():
        entry: dict[str, Any] = {}

        if definition.auth_mode == AuthMode.LOCAL_NO_AUTH:
            entry = _classify_ollama(definition, resolver_map, doctor_map, reserved)

        elif definition.auth_mode == AuthMode.CLI_OAUTH:
            native = detect_native_login(definition)
            if definition.cliproxy_provider_id is None:
                # Future adapter (e.g. github_copilot) — no bridge path yet.
                entry = {
                    "provider_id": pid,
                    "family": definition.family,
                    "auth_mode": AuthMode.CLI_OAUTH,
                    "native_login_detected": native.detected,
                    "bridge_status": "future_adapter",
                    "bridge_detail": definition.description
                    or "Requiere adapter futuro.",
                    "usable_now": False,
                    "models": [],
                    "models_status": "future_adapter",
                    "next_action": "requires_adapter",
                    "status": DetectionStatus.REQUIRES_ADAPTER,
                    "status_detail": definition.description
                    or "Requiere adapter futuro.",
                    "credential_detected": native.detected,
                    "credential_path_safe": native.credential_path,
                    "email": None,
                    "last_refresh": None,
                }
            else:
                entry = _classify_cli_oauth(
                    definition, native, bridge_status, live_bridge_models
                )

        elif definition.auth_mode == AuthMode.DIRECT_KEY:
            if pid == "mimo_code":
                # Sprint C: desde jun 2026 MiMo Code SÍ escribe auth.json
                # (entrada "xiaomi" con key + metadata.base_url regional) al
                # loguearse con la suscripción. Se detecta como la familia
                # OpenCode; si no hay login, cae al detector viejo (mensaje
                # de formato/adapter, nunca mudo).
                oc = detect_opencode_family(definition, probe_online=probe_online)
                if oc.credential_detected:
                    entry = _classify_opencode_family(oc)
                else:
                    result = detect_mimo(definition)
                    entry = _classify_mimo(result)
            else:
                result = detect_opencode_family(definition, probe_online=probe_online)
                entry = _classify_opencode_family(result)

        elif definition.auth_mode == AuthMode.STATIC_API_KEY:
            entry = _classify_static_api_key(definition, environment)

        elif definition.auth_mode == AuthMode.OPENAI_COMPATIBLE:
            entry = _classify_openai_compatible(definition, environment)

        else:
            entry = {
                "provider_id": pid,
                "family": definition.family,
                "auth_mode": definition.auth_mode,
                "status": DetectionStatus.NOT_CONFIGURED,
                "status_detail": f"auth_mode {definition.auth_mode} sin detector.",
                "usable_now": False,
                "models": [],
                "next_action": None,
                "native_login_detected": False,
                "bridge_status": "unknown",
                "bridge_detail": "",
                "credential_detected": False,
                "credential_path_safe": None,
                "email": None,
                "last_refresh": None,
            }

        # ── v1 backward-compat fields (Rust panel reads these) ──
        entry.setdefault("id", pid)
        entry.setdefault("display_name", definition.display_name)
        entry.setdefault("category", definition.category)
        entry.setdefault("connection_type", definition.auth_mode)
        entry.setdefault("recommended", definition.recommended)
        entry.setdefault("description", definition.description)
        entry.setdefault("models_detected", entry.get("models", []))
        # credential_fingerprint for non-OpenCode may be unset; keep None.

        providers.append(entry)

    # ── Stored keys (Catálogo one-step logins) — persistence layer ──────────
    # Keys guardadas por provider_login.py (~/.nexum/api_keys.json). Un provider
    # con key almacenada es USABLE entre reinicios; con --probe-online se
    # re-valida la key y se refresca la lista de modelos.
    store = key_store if key_store is not None else KeyStore()
    stored_ids = set(store.stored_provider_ids())
    for spid in sorted(stored_ids):
        stored = store.get_stored(spid)
        centry = get_catalog_entry(spid)
        if stored is None or centry is None:
            continue
        models = list(stored.models)
        entry_dict: dict[str, Any] | None = None
        if probe_online:
            raw_key = store.get_key(spid)
            if centry.needs_base_url or not centry.models_endpoint:
                endpoint = stored.base_url.rstrip("/") + "/models"
            else:
                endpoint = centry.models_endpoint
            probe = probe_api_key(endpoint, raw_key or "", stored.protocol)
            if probe.success:
                models = probe.models
                store.store(
                    spid, raw_key or "", stored.base_url, stored.protocol, models
                )
            elif probe.error_code in (401, 403):
                entry_dict = build_usable_entry(
                    centry, stored.base_url, [], stored.fingerprint
                )
                entry_dict.update(
                    {
                        "usable_now": False,
                        "status": DetectionStatus.PROBE_FAILED,
                        "models_status": "probe_failed",
                        "next_action": "reingresar_api_key",
                        "status_detail": (
                            "La key almacenada fue rechazada (401/403). "
                            "Reingresala desde el Catálogo de /provedor."
                        ),
                        "bridge_detail": "Key almacenada rechazada por el proveedor.",
                        "model_policy": {},
                        "models_detected": [],
                    }
                )
            # Network errors: keep the stored models (offline ≠ key inválida).
        if entry_dict is None:
            entry_dict = build_usable_entry(
                centry, stored.base_url, models, stored.fingerprint
            )
        providers = [
            p for p in providers if p.get("provider_id") != spid and p.get("id") != spid
        ]
        providers.append(entry_dict)

    # ── Catálogo (pre-configured one-step-login providers) ──────────────────
    # Dedup rule (ADR-044 cierre): a provider already in "Tus proveedores"
    # (usable or with credential detected) never appears in the Catálogo.
    detected_ids = {
        p.get("provider_id")
        for p in providers
        if p.get("usable_now") or p.get("credential_detected")
    }
    catalog_rows: list[dict[str, Any]] = []
    for cpid, centry in CATALOG_PROVIDERS.items():
        if cpid in detected_ids or cpid in stored_ids:
            continue
        catalog_rows.append(
            {
                "provider_id": cpid,
                "display_name": centry.display_name,
                "base_url": centry.base_url,
                "protocol": centry.protocol,
                "key_env_hint": centry.key_env_hint,
                "needs_base_url": centry.needs_base_url,
                "static_models": list(centry.static_models),
            }
        )

    # Notes.
    if bridge_status.status == DetectionStatus.BRIDGE_NOT_INSTALLED:
        notes.append(
            "CLIProxyAPI no está instalado. Claude/Codex/Gemini muestran "
            "'login nativo detectado' pero requieren instalar CLIProxyAPI para "
            "activar el puente. Instalalo manualmente: paru -S cli-proxy-api-bin"
        )
    for p in providers:
        if p.get("provider_id") == "opencode_zen" and p.get("usable_now"):
            notes.append(
                "OpenCode Zen usable vía clave directa (auth.json). No es lo "
                "mismo que el login OAuth de la CLI OpenCode."
            )

    recommended_id = resolver.get("recommended_provider") or "ollama_local"
    active_provider_id = _detect_active_provider_id(cli_root)

    return {
        "version": VERSION,
        "catalog_version": VERSION,  # v1 alias
        "generated_at": _now_iso(),
        "doctor_input": str(doctor_path),
        "resolver_input": str(resolver_path),
        "cli_proxy_api": bridge_status.to_dict(),
        "recommended_provider_id": recommended_id,
        "active_provider_id": active_provider_id,
        "reserved_models": reserved_entries(),
        "providers": providers,
        "catalog": catalog_rows,
        "notes": notes,
    }


def _write_json(catalog: dict[str, Any], out_path: Path) -> None:
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with out_path.open("w", encoding="utf-8") as fh:
        json.dump(catalog, fh, indent=2, ensure_ascii=False)
        fh.write("\n")


# ─── Rendering (text) ─────────────────────────────────────────────────────────


def _render_text(catalog: dict[str, Any]) -> str:
    lines: list[str] = ["Nexum — Catálogo de Proveedores v2", ""]
    b = catalog.get("cli_proxy_api", {})
    lines.append(
        f"CLIProxyAPI: installed={b.get('installed')} "
        f"running={b.get('running')} status={b.get('status')}"
    )
    lines.append("")

    def glyph(p: dict[str, Any]) -> str:
        if p.get("usable_now"):
            return "✓"
        if p.get("native_login_detected"):
            return "◐"
        return "○"

    for p in catalog.get("providers", []):
        lines.append(
            f"{glyph(p)} {p.get('display_name', p.get('provider_id')):<22} "
            f"({p.get('family')}) {p.get('status')}"
        )
        if p.get("next_action"):
            lines.append(f"    next_action: {p['next_action']}")
    return "\n".join(lines)


# ─── CLI ──────────────────────────────────────────────────────────────────────


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Nexum Provider Catalog v2 — ADR-044 detection."
    )
    here = Path(__file__).resolve().parent
    cache_home = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    cache_dir = cache_home / "nexum/providers"

    parser.add_argument(
        "--doctor",
        default=str(cache_dir / DEFAULT_DOCTOR),
        help="Path to provider-doctor-output.json.",
    )
    parser.add_argument(
        "--resolver",
        default=str(cache_dir / DEFAULT_RESOLVER),
        help="Path to provider-resolver-output.json.",
    )
    parser.add_argument(
        "--output",
        default=str(here / "provider-catalog-output.json"),
        help="Path to write provider-catalog-output.json.",
    )
    parser.add_argument(
        "--probe-online",
        action="store_true",
        help="Enable live /models probes for OpenCode-family + static API-key "
        "providers (uses their keys against provider endpoints).",
    )
    parser.add_argument("--json", action="store_true", help="Print JSON to stdout.")
    args = parser.parse_args(argv)

    try:
        catalog = build_catalog(
            Path(args.doctor),
            Path(args.resolver),
            probe_online=args.probe_online,
        )
    except (FileNotFoundError, ValueError, json.JSONDecodeError) as exc:
        print(f"[catalog] error: {exc}", file=sys.stderr)
        return 2

    try:
        _write_json(catalog, Path(args.output))
    except OSError as exc:
        print(
            f"[catalog] warning: could not write {args.output}: {exc}", file=sys.stderr
        )

    if args.json:
        print(json.dumps(catalog, indent=2, ensure_ascii=False))
    else:
        print(_render_text(catalog))
    return 0


if __name__ == "__main__":
    sys.exit(main())
