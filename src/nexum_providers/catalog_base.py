"""Sanitizador del catálogo base de providers (regresión rc.4).

Produce un catálogo BASE genérico y sin secretos a partir de un catálogo
generado, apto para viajar dentro del artefacto instalado:

  - elimina TODO secreto/dato de usuario (credential_fingerprint, email,
    credential_path_safe, bridge_detail con rutas, cli_proxy_api con puerto/
    binary_path, active/recommended específicos del usuario);
  - resetea el estado live por provider a "no configurado" (el reconcile del
    producto instalado lo actualiza contra el CLIProxyAPI real);
   - conserva cada provider soportado por el registro canónico, incluso si es
     un servicio de terceros; el estado base nunca afirma conexión ni uso.

No imprime secretos. No requiere red ni checkout.
"""

from __future__ import annotations

import json
from typing import Any

# Campos por-provider que contienen estado/secretos de usuario: se resetean.
_USER_STATE_FIELDS = {
    "credential_fingerprint": None,
    "credential_path_safe": None,
    "credential_detected": False,
    "native_login_detected": False,
    "usable_now": False,
    "bridge_status": "not_configured",
    "bridge_detail": "",
    "status": "not_configured",
    "status_detail": "No configurado en esta instalación. Conectá desde /proveedor.",
    "models_status": "unknown",
    "email": None,
    "last_refresh": None,
    "recommended": False,
    "next_action": "connect",
}


def sanitize_provider(p: dict[str, Any]) -> dict[str, Any]:
    """Devuelve una copia del provider sin secretos y con estado reseteado,
    preservando la metadata estructural (id, name, category, base_url, models,
    capacidades, context)."""
    q = dict(p)
    for k, v in _USER_STATE_FIELDS.items():
        if k in q:
            q[k] = v
    # Los modelos ESTRUCTURALES se preservan (identificadores, no secretos).
    return q


def build_base_catalog(generated: dict[str, Any]) -> dict[str, Any]:
    """Construye el catálogo base sanitizado desde un catálogo generado."""
    providers = generated.get("providers", [])
    active = [sanitize_provider(p) for p in providers]

    return {
        "schema_version": 2,
        "catalog_kind": "base",
        "version": generated.get("version", "base"),
        "catalog_version": generated.get("catalog_version", "base"),
        "generated_at": "base",
        # Estado del bridge: reseteado (lo llena el reconcile instalado).
        "cli_proxy_api": {"installed": False, "running": False, "status": "unknown"},
        # Sin selección específica de usuario en el base.
        "recommended_provider_id": None,
        "active_provider_id": None,
        "reserved_models": generated.get("reserved_models", []),
        "providers": active,
        "notes": [
            "Catálogo BASE sin secretos, shippeado en el artefacto instalado.",
            "El reconcile del producto instalado lo actualiza con el estado live del bridge.",
            "Cada provider soportado conserva estado no configurado hasta evidencia local.",
        ],
    }


def main(argv: list[str]) -> int:
    if len(argv) < 3:
        print("uso: catalog_base.py <catalogo_generado.json> <salida_base.json>")
        return 2
    src, dst = argv[1], argv[2]
    with open(src, encoding="utf-8") as f:
        generated = json.load(f)
    base = build_base_catalog(generated)
    with open(dst, "w", encoding="utf-8") as f:
        json.dump(base, f, indent=2, ensure_ascii=False)
    print(f"base catalog: {len(base['providers'])} providers soportados")
    return 0


if __name__ == "__main__":
    import sys

    raise SystemExit(main(sys.argv))
