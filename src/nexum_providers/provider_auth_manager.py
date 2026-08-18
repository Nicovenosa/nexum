"""Nexum Provider Auth Manager — capa unificada de estado de autenticación.

Spec AUTOLOGIN TOTAL (2026-07-06), Parte 1: un solo lugar que sabe, para cada
provider, QUÉ auth_mode usa, SI ya está autenticado, y CUÁL es la próxima
acción accionable. Consolida las fuentes que ya existen (no las duplica):

  - provider-catalog-output.json  → detección (usable_now, bridge_status,
                                    native_login_detected, models)
  - KeyStore (~/.nexum/api_keys.json, 600) → keys guardadas por login manual
  - env vars (ZAI_CODING_API_KEY, …)       → autologin por entorno
  - bridge_supervisor                       → salud de CLIProxyAPI (puerto)

Estados expuestos (subset ESTÁTICO de la máquina de estados del spec — los
estados transitorios login_in_progress/browser_opened/callback_* viven en el
job del TUI, no acá):

    usable               provider listo para mandar prompts ya
    credential_detected  hay credencial (key/login nativo) pero falta validar
    login_required       hay flujo de login disponible, falta ejecutarlo
    not_configured       sin credencial y sin flujo automático detectado
    upstream_limitation  el flujo existe pero el upstream no lo soporta
    unusable             detectado pero sin camino a usable (p.ej. sin puente)

Auth modes (spec Parte 1):

    api_key | env_api_key | secure_key_store | native_cli_login |
    cliproxyapi_oauth | cliproxyapi_management | local_callback_oauth |
    external_subscription | local_no_auth | unsupported_safe

Seguridad: NUNCA imprime keys/tokens/state. Solo fingerprints enmascarados
(first2…last4) que ya produce el KeyStore. Stdlib only.

CLI:
    provider_auth_manager.py --status            → JSON con todos los providers
    provider_auth_manager.py --status <provider> → JSON de uno solo
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from nexum_providers.catalog_providers import CATALOG_PROVIDERS
from nexum_providers.catalog_gen.provider_registry import PROVIDER_REGISTRY
from nexum_providers.key_store import KeyStore
from nexum_providers.provider_auto_login import AUTO_LOGIN_PROVIDERS

# ─── Definición de auth por provider ─────────────────────────────────────────
#
# auth_modes en orden de preferencia: el primero que tenga credencial "gana".
# Los providers OAuth del puente llevan además su fallback oficial por API key
# (spec Partes 2-4: "si OAuth falla, ofrecer alternativa oficial").

PROVIDER_AUTH: dict[str, dict[str, Any]] = {
    "codex_cli": {
        "display_name": "Codex / OpenAI",
        "auth_modes": ["cliproxyapi_oauth", "native_cli_login", "api_key"],
        "api_key_fallback": "openai",
    },
    "claude_code": {
        "display_name": "Claude Code / Anthropic",
        "auth_modes": ["cliproxyapi_oauth", "native_cli_login", "api_key"],
        "api_key_fallback": "anthropic_api_key",
    },
    "gemini_cli": {
        "display_name": "Gemini CLI / Google",
        "auth_modes": ["cliproxyapi_oauth", "native_cli_login", "api_key"],
        "api_key_fallback": "google_api_key",
    },
    "glm_coding_plan": {
        "display_name": "Z.ai / GLM Coding Plan",
        "auth_modes": ["env_api_key", "api_key"],
        "env_var": "ZAI_CODING_API_KEY",
    },
    "ollama_local": {
        "display_name": "Ollama Local",
        "auth_modes": ["local_no_auth"],
    },
    "opencode_zen": {
        "display_name": "OpenCode Zen",
        "auth_modes": ["external_subscription"],
    },
    "opencode_go": {
        "display_name": "OpenCode Go",
        "auth_modes": ["external_subscription"],
    },
    "mimo_code": {
        "display_name": "MiMo Code",
        "auth_modes": ["external_subscription"],
    },
}

# Solo los IDs que existen también en el registry canónico soportan el login
# one-step. Las filas históricas del catálogo no deben filtrarse a status_all.
for _pid in set(CATALOG_PROVIDERS) & set(PROVIDER_REGISTRY):
    PROVIDER_AUTH.setdefault(
        _pid,
        {
            "display_name": CATALOG_PROVIDERS[_pid].display_name,
            "auth_modes": ["api_key"],
        },
    )

# El status manager cubre exactamente el registry productivo. Los providers sin
# login implementado se exponen de forma segura, sin inventar conectividad.
for _definition in PROVIDER_REGISTRY.values():
    PROVIDER_AUTH.setdefault(
        _definition.provider_id,
        {
            "display_name": _definition.display_name,
            "auth_modes": ["unsupported_safe"],
        },
    )


@dataclass
class AuthStatus:
    """Estado de auth de UN provider, JSON-safe, sin secrets."""

    provider_id: str
    display_name: str
    auth_mode: str  # el modo ACTIVO (o el preferido si no hay credencial)
    state: str  # usable | credential_detected | login_required |
    #             not_configured | upstream_limitation | unusable
    detail: str  # una línea humana, accionable
    next_action: str | None  # "connect_bridge" | "enter_api_key" |
    #                          "native_login" | None
    fingerprint: str | None = None  # enmascarado (first2…last4), nunca la key
    models: list[str] = field(default_factory=list)


# ─── Fuentes ──────────────────────────────────────────────────────────────────


def _default_catalog_path() -> Path:
    data_home = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share")
    return data_home / "nexum/providers/provider-catalog-live.json"


def load_catalog(catalog_path: Path | None = None) -> dict[str, dict[str, Any]]:
    """providers del catálogo indexados por provider_id. {} si no existe."""
    path = catalog_path or _default_catalog_path()
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    out: dict[str, dict[str, Any]] = {}
    for p in doc.get("providers", []):
        pid = p.get("provider_id") or p.get("id")
        if isinstance(pid, str) and pid:
            out[pid] = p
    return out


# ─── Resolución de estado ─────────────────────────────────────────────────────


def resolve_status(
    provider_id: str,
    *,
    catalog: dict[str, dict[str, Any]],
    key_store: KeyStore,
    env: dict[str, str],
    bridge_running: bool,
) -> AuthStatus:
    """Estado estático de auth de un provider (sin red, sin side-effects)."""
    meta = PROVIDER_AUTH.get(provider_id, {})
    display = str(meta.get("display_name") or provider_id)
    modes: list[str] = list(meta.get("auth_modes") or ["api_key"])
    preferred = modes[0]
    cat = catalog.get(provider_id, {})

    def status(
        state: str,
        detail: str,
        *,
        auth_mode: str = preferred,
        next_action: str | None = None,
        fingerprint: str | None = None,
        models: list[str] | None = None,
    ) -> AuthStatus:
        return AuthStatus(
            provider_id=provider_id,
            display_name=display,
            auth_mode=auth_mode,
            state=state,
            detail=detail,
            next_action=next_action,
            fingerprint=fingerprint,
            models=models or [],
        )

    if "unsupported_safe" in modes:
        return status(
            "unsupported",
            f"{display}: no tiene un flujo de login soportado en esta versión.",
            auth_mode="unsupported_safe",
        )

    # 1) Catálogo dice usable → usable (la fuente ya validó con probe real).
    if cat.get("usable_now"):
        stored = key_store.get_stored(provider_id)
        mode = "secure_key_store" if stored else preferred
        return status(
            "usable",
            f"{display} usable ahora.",
            auth_mode=mode,
            fingerprint=stored.fingerprint if stored else None,
            models=[m for m in (cat.get("models") or []) if isinstance(m, str)],
        )

    # 2) Key en el KeyStore pero catálogo aún no la reflejó → credencial
    #    detectada (falta regen/probe, no falta login).
    if key_store.has_key(provider_id):
        stored = key_store.get_stored(provider_id)
        return status(
            "credential_detected",
            f"{display}: API key guardada; falta refrescar el catálogo.",
            auth_mode="secure_key_store",
            fingerprint=stored.fingerprint if stored else None,
        )

    # 3) Env var del autologin presente → credencial detectada (el autologin
    #    del arranque la valida con probe y la pasa al KeyStore).
    env_var = meta.get("env_var") or AUTO_LOGIN_PROVIDERS.get(provider_id)
    if env_var and (env.get(env_var) or "").strip():
        return status(
            "credential_detected",
            f"{display}: {env_var} presente; el autologin la valida al arrancar.",
            auth_mode="env_api_key",
        )

    # 3.5) Cuenta puenteada pero con límite de uso del plan (temporal).
    #      Re-loguear no lo arregla: no mandamos al usuario al puente.
    if cat.get("bridge_status") == "rate_limited":
        return status(
            "credential_detected",
            f"{display}: conectado y autenticado; el plan alcanzó su límite "
            "de uso. Se recupera solo — no hace falta re-loguear.",
            models=[m for m in (cat.get("models") or []) if isinstance(m, str)],
        )

    # 4) Flujo OAuth por puente (Codex/Claude/Gemini).
    if "cliproxyapi_oauth" in modes:
        fallback = meta.get("api_key_fallback")
        if not bridge_running:
            return status(
                "unusable",
                "CLIProxyAPI no está corriendo: el login OAuth necesita el "
                "puente. Alternativa oficial: conectar por API key.",
                next_action="enter_api_key" if fallback else "connect_bridge",
            )
        if cat.get("native_login_detected"):
            return status(
                "credential_detected",
                f"{display}: login nativo detectado; conectá el puente para "
                "usarlo desde Nexum.",
                next_action="connect_bridge",
            )
        return status(
            "login_required",
            f"{display}: Enter inicia el login OAuth (el puente captura el "
            "callback). Alternativa: API key.",
            next_action="connect_bridge",
        )

    # 5) API key manual disponible (Catálogo one-step login).
    if "api_key" in modes or "env_api_key" in modes:
        return status(
            "login_required",
            f"{display}: requiere API key. Enter → conectar con API key.",
            auth_mode="api_key",
            next_action="enter_api_key",
        )

    # 6) Local / suscripción externa no detectada.
    if "local_no_auth" in modes:
        return status(
            "not_configured",
            f"{display}: server local no detectado.",
        )
    if "external_subscription" in modes:
        return status(
            "not_configured",
            f"{display}: suscripción externa no detectada en esta máquina.",
        )

    return status("not_configured", f"{display}: sin flujo de auth conocido.")


def status_all(
    *,
    catalog_path: Path | None = None,
    key_store: KeyStore | None = None,
    env: dict[str, str] | None = None,
    bridge_running: bool | None = None,
) -> list[AuthStatus]:
    """Estado de auth de todos los providers conocidos por el manager."""
    catalog = load_catalog(catalog_path)
    store = key_store or KeyStore()
    env_map = env if env is not None else dict(os.environ)
    if bridge_running is None:
        bridge_running = _bridge_port_open()
    return [
        resolve_status(
            pid,
            catalog=catalog,
            key_store=store,
            env=env_map,
            bridge_running=bridge_running,
        )
        for pid in PROVIDER_AUTH
    ]


def _bridge_port_open() -> bool:
    """Salud del puente sin importar bridge_supervisor (evita ciclo)."""
    import socket

    try:
        with socket.create_connection(("127.0.0.1", 8317), timeout=0.5):
            return True
    except OSError:
        return False


# ─── CLI ──────────────────────────────────────────────────────────────────────


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Estado unificado de auth de providers (sin secrets)."
    )
    parser.add_argument(
        "--status",
        nargs="?",
        const="__all__",
        metavar="PROVIDER_ID",
        help="Estado de todos los providers, o de uno solo.",
    )
    parser.add_argument(
        "--catalog",
        default=str(_default_catalog_path()),
        help="Path a provider-catalog-output.json.",
    )
    args = parser.parse_args(argv)

    if not args.status:
        parser.print_help()
        return 1

    statuses = status_all(catalog_path=Path(args.catalog))
    if args.status != "__all__":
        statuses = [s for s in statuses if s.provider_id == args.status]
        if not statuses:
            print(
                json.dumps(
                    {"ok": False, "message": f"Provider desconocido: {args.status}"}
                )
            )
            return 1
    # stdout: SOLO estados sanitizados (fingerprints enmascarados, sin keys).
    print(
        json.dumps(
            {"ok": True, "providers": [asdict(s) for s in statuses]},
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
