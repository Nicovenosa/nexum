"""Nexum Provider Login — one-step login CLI for Catálogo providers (ADR-044 cierre).

Invoked by the nexum-tui `/provedor` panel when the user pastes an API key on a
Catálogo row. Protocol (all JSON, single line):

    stdin:  {"provider_id": "...", "api_key": "...", "base_url": "..."?}
    stdout: {"ok": true,  "provider_id", "display_name", "base_url", "protocol",
             "models": [...], "fingerprint"}
          | {"ok": false, "error_kind": "invalid_key"|"network"|"http"|"input",
             "error_code": int|null, "message": "..."}

On success it ALSO:
  1. Stores the key in the KeyStore (~/.nexum/api_keys.json, 600).
  2. Upserts the provider as USABLE into provider-catalog-output.json so
     /provedor and /modelo reflect it immediately (no restart).

Security: the API key travels ONLY via stdin (never argv, never env, never
logged). stdout carries only the masked fingerprint.

Stdlib only. No Nexum runtime imports.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from nexum_providers.catalog_providers import CatalogProviderEntry, get_catalog_entry
from nexum_providers.key_store import KeyStore
from nexum_providers.probe_validator import ProbeResult, probe_api_key


def _default_catalog_path() -> Path:
    data_home = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share")
    return data_home / "nexum/providers/provider-catalog-live.json"


# ─── Catalog upsert ───────────────────────────────────────────────────────────


def build_usable_entry(
    entry: CatalogProviderEntry,
    base_url: str,
    models: list[str],
    fingerprint: str,
) -> dict[str, Any]:
    """Build a v1+v2 catalog provider dict for a validated key (usable_now)."""
    return {
        # v2 (ADR-044)
        "provider_id": entry.provider_id,
        "family": entry.display_name,
        "auth_mode": "static_api_key",
        "native_login_detected": True,
        "bridge_status": "direct_key",
        "bridge_detail": "API key validada por probe en vivo (login de un paso).",
        "usable_now": True,
        "models": models,
        "models_status": "ok",
        "next_action": None,
        "email": None,
        "last_refresh": None,
        # v1 backward-compat (Rust panel)
        "id": entry.provider_id,
        "display_name": entry.display_name,
        "category": "cloud",
        "connection_type": "static_api_key",
        "status": "usable",
        "status_detail": f"{entry.display_name} conectado con API key validada.",
        "recommended": False,
        "credential_detected": True,
        "credential_fingerprint": fingerprint,
        "credential_path_safe": "~/.nexum/api_keys.json",
        "base_url_detected": base_url,
        "description": None,
        "models_detected": models,
        "model_policy": {
            "user_facing_models": models,
            "user_facing_count": len(models),
            "reserved_internal_count": 0,
            "all_models_reserved": False,
            "selectable_models": models,
        },
    }


def upsert_catalog_entry(catalog_path: Path, provider_entry: dict[str, Any]) -> bool:
    """Insert/replace a provider in the catalog JSON and drop it from the
    `catalog` (pre-configured) array. Atomic write. Returns True on success."""
    try:
        doc = json.loads(catalog_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        # No catalog yet — create a minimal v2 doc so the TUI still sees it.
        doc = {"version": "2", "catalog_version": "2", "providers": [], "notes": []}

    pid = provider_entry["provider_id"]
    providers = [
        p
        for p in doc.get("providers", [])
        if p.get("provider_id") != pid and p.get("id") != pid
    ]
    providers.append(provider_entry)
    doc["providers"] = providers

    if isinstance(doc.get("catalog"), list):
        doc["catalog"] = [c for c in doc["catalog"] if c.get("provider_id") != pid]

    try:
        catalog_path.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(
            dir=str(catalog_path.parent), prefix=".catalog-", suffix=".json"
        )
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(doc, fh, indent=2, ensure_ascii=False)
            fh.write("\n")
        os.replace(tmp, catalog_path)
        return True
    except OSError:
        return False


# ─── Login flow ───────────────────────────────────────────────────────────────


def run_login(
    request: dict[str, Any],
    *,
    catalog_path: Path,
    key_store: KeyStore | None = None,
    prober=probe_api_key,
) -> dict[str, Any]:
    """Validate + store + upsert. Pure-ish (injectable) for tests."""
    provider_id = str(request.get("provider_id", "")).strip()
    api_key = str(request.get("api_key", "")).strip()
    base_url_in = str(request.get("base_url", "") or "").strip()

    entry = get_catalog_entry(provider_id)
    if entry is None:
        return {
            "ok": False,
            "error_kind": "input",
            "error_code": None,
            "message": f"Proveedor desconocido en el catálogo: {provider_id}",
        }
    if not api_key:
        return {
            "ok": False,
            "error_kind": "input",
            "error_code": None,
            "message": "API key vacía.",
        }

    if entry.needs_base_url:
        if not base_url_in or not base_url_in.startswith("http"):
            return {
                "ok": False,
                "error_kind": "input",
                "error_code": None,
                "message": "Este proveedor requiere la base URL de tu dashboard.",
            }
        base_url = base_url_in.rstrip("/")
        models_endpoint = base_url + "/models"
    else:
        base_url = entry.base_url
        models_endpoint = entry.models_endpoint

    result: ProbeResult = prober(models_endpoint, api_key, entry.protocol)

    if not result.success:
        if result.no_models:
            # Auth OK (200) pero el endpoint no devolvió modelos: la key
            # funciona pero el plan/cuenta no expone modelos seleccionables.
            # No guardamos la key: el provider no es usable sin modelos.
            kind = "model_list_failed"
        elif result.error_code in (401, 403):
            kind = "invalid_key"
        elif result.error_code is not None:
            kind = "http"
        else:
            kind = "network"
        return {
            "ok": False,
            "error_kind": kind,
            "error_code": result.error_code,
            "message": result.status_label,
        }

    models = result.models or list(entry.static_models)
    store = key_store or KeyStore()
    stored = store.store(
        provider_id=entry.provider_id,
        api_key=api_key,
        base_url=base_url,
        protocol=entry.protocol,
        models=models,
    )
    upsert_catalog_entry(
        catalog_path,
        build_usable_entry(entry, base_url, models, stored.fingerprint),
    )
    return {
        "ok": True,
        "provider_id": entry.provider_id,
        "display_name": entry.display_name,
        "base_url": base_url,
        "protocol": entry.protocol,
        "models": models,
        "fingerprint": stored.fingerprint,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Nexum one-step provider login (reads JSON request on stdin)."
    )
    parser.add_argument(
        "--catalog",
        default=str(_default_catalog_path()),
        help="Path to provider-catalog-output.json to upsert on success.",
    )
    args = parser.parse_args(argv)

    try:
        request = json.loads(sys.stdin.read())
    except (ValueError, OSError):
        print(
            json.dumps(
                {
                    "ok": False,
                    "error_kind": "input",
                    "error_code": None,
                    "message": "Request JSON inválido en stdin.",
                }
            )
        )
        return 1

    outcome = run_login(request, catalog_path=Path(args.catalog))
    print(json.dumps(outcome, ensure_ascii=False))
    return 0 if outcome.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main())
