"""Nexum Provider Auto-Login — login automático desde env vars (GLM Coding Plan).

Permite que un usuario recién instalado deje GLM Coding Plan usable sin
pasar por /provedor: alcanza con `export ZAI_CODING_API_KEY=...` antes de
lanzar Nexum. Al arranque, el TUI invoca este script; si la key probea
bien, se guarda en el KeyStore y se upserta el catálogo como usable_now.

Protocolo (JSON, una línea, sin args sensibles):

    argv:  (ninguno — la key se lee del ENV, nunca de argv)
    env:   ZAI_CODING_API_KEY=<key>   (la única fuente)
    stdout (éxito):
        {"ok": true, "provider_id": "glm_coding_plan", "source": "env_autologin",
         "models": [...], "fingerprint": "sk…x4Kp", "display_name": "...",
         "base_url": "...", "protocol": "openai"}
    stdout (no-op / fallo):
        {"ok": false, "reason": "no_env_var" | "already_logged_in" |
                                "invalid_key" | "model_list_failed" | "network" |
                                "http" | "unknown_provider",
         "error_code": int|null, "message": "..."}

Reglas de seguridad (heredadas de provider_login.py):
  - La API key viaja SOLO por env (nunca argv, nunca stdin, nunca logueada).
  - stdout lleva únicamente el fingerprint enmascarado (first2…last4).
  - Si el provider ya está logeado en el KeyStore, no hace nada (no re-probea).

Stdlib only. No Nexum runtime imports.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from nexum_providers.catalog_providers import get_catalog_entry
from nexum_providers.key_store import KeyStore
from nexum_providers.probe_validator import ProbeResult, probe_api_key
from nexum_providers.provider_login import (
    build_usable_entry,
    upsert_catalog_entry,
)

# Mapping: catalog provider_id → env var que dispara el auto-login.
# Hoy solo GLM Coding Plan; la tabla permite sumar más providers con el
# mismo patrón sin tocar la lógica.
AUTO_LOGIN_PROVIDERS: dict[str, str] = {
    "glm_coding_plan": "ZAI_CODING_API_KEY",
}


def _default_catalog_path() -> Path:
    data_home = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share")
    return data_home / "nexum/providers/provider-catalog-live.json"


def _outcome_ok(
    provider_id: str,
    entry_models: list[str],
    fingerprint: str,
    base_url: str,
    protocol: str,
    display_name: str,
) -> dict[str, Any]:
    return {
        "ok": True,
        "provider_id": provider_id,
        "source": "env_autologin",
        "models": entry_models,
        "fingerprint": fingerprint,
        "display_name": display_name,
        "base_url": base_url,
        "protocol": protocol,
    }


def _outcome_fail(
    reason: str,
    *,
    error_code: int | None = None,
    message: str = "",
) -> dict[str, Any]:
    return {
        "ok": False,
        "reason": reason,
        "error_code": error_code,
        "message": message,
    }


def run_auto_login(
    provider_id: str,
    *,
    env: dict[str, str] | None = None,
    catalog_path: Path,
    key_store: KeyStore | None = None,
    prober=probe_api_key,
) -> dict[str, Any]:
    """Intenta auto-logear un provider desde su env var. Puro-inyectable.

    Args:
        provider_id: catalog provider id (de AUTO_LOGIN_PROVIDERS).
        env: entorno a consultar (default os.environ). Inyectable para tests.
        catalog_path: path a provider-catalog-output.json para upsert.
        key_store: KeyStore inyectable para tests.
        prober: función probe inyectable para tests.

    Returns:
        Dict JSON-serializable con el contrato documentado arriba.
    """
    env_map = env if env is not None else dict(os.environ)

    entry = get_catalog_entry(provider_id)
    if entry is None or provider_id not in AUTO_LOGIN_PROVIDERS:
        return _outcome_fail(
            "unknown_provider",
            message=f"Auto-login no soporta el provider: {provider_id}",
        )

    env_var = AUTO_LOGIN_PROVIDERS[provider_id]
    api_key = (env_map.get(env_var) or "").strip()
    if not api_key:
        return _outcome_fail("no_env_var", message=f"Sin {env_var} en el entorno.")

    # Si ya está logeado, no re-probeamos: evita gasto de red y re-login
    # innecesario en cada arranque. El usuario puede forzar re-login desde
    # /provedor si quiere rotar la key.
    store = key_store or KeyStore()
    if store.has_key(provider_id):
        stored = store.get_stored(provider_id)
        return {
            "ok": False,
            "reason": "already_logged_in",
            "provider_id": provider_id,
            "fingerprint": stored.fingerprint if stored else None,
            "message": f"{provider_id} ya está logeado.",
        }

    # Probe en vivo (mismo path que el login manual).
    result: ProbeResult = prober(entry.models_endpoint, api_key, entry.protocol)

    if not result.success:
        if result.no_models:
            kind = "model_list_failed"
        elif result.error_code in (401, 403):
            kind = "invalid_key"
        elif result.error_code is not None:
            kind = "http"
        else:
            kind = "network"
        return _outcome_fail(
            kind,
            error_code=result.error_code,
            message=result.status_label,
        )

    models = result.models or list(entry.static_models)
    stored = store.store(
        provider_id=entry.provider_id,
        api_key=api_key,
        base_url=entry.base_url,
        protocol=entry.protocol,
        models=models,
    )
    upsert_catalog_entry(
        catalog_path,
        build_usable_entry(entry, entry.base_url, models, stored.fingerprint),
    )
    return _outcome_ok(
        provider_id=entry.provider_id,
        entry_models=models,
        fingerprint=stored.fingerprint,
        base_url=entry.base_url,
        protocol=entry.protocol,
        display_name=entry.display_name,
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Nexum auto-login desde env vars. Lee ZAI_CODING_API_KEY del "
            "entorno y, si probea OK, deja glm_coding_plan usable."
        )
    )
    parser.add_argument(
        "--provider",
        default="glm_coding_plan",
        help="Catalog provider id (default: glm_coding_plan).",
    )
    parser.add_argument(
        "--catalog",
        default=str(_default_catalog_path()),
        help="Path a provider-catalog-output.json para upsert.",
    )
    args = parser.parse_args(argv)

    outcome = run_auto_login(
        args.provider,
        catalog_path=Path(args.catalog),
    )
    # CRÍTICO: stdout SOLO lleva el resultado; la key NUNCA se imprime.
    print(json.dumps(outcome, ensure_ascii=False))
    # Exit 0 siempre: un fallo/no-op de auto-login no debe abortar el
    # arranque del TUI. El usuario puede loguear manualmente vía /provedor.
    return 0


if __name__ == "__main__":
    sys.exit(main())
