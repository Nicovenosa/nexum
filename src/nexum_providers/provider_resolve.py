"""Nexum Provider Resolve — resuelve credenciales LOCALES de un provider (fix Bug 2).

Cuando el usuario elige en /modelo un modelo de un provider que todavía no
tiene ProviderConfig en settings.json (p.ej. OpenCode Zen recién detectado),
el TUI invoca este script para obtener base_url + api_key y crear la config.
Sin esto, LlmProvider::from_config devuelve None y el prompt sigue yendo al
provider anterior (el bug de "elijo deepseek y responde Ollama").

Uso:
    python3 provider_resolve.py <provider_id>
    stdout: {"ok": true, "provider_id", "display_name", "base_url",
             "api_key", "protocol"}
          | {"ok": false, "message": "..."}

Fuentes de credenciales (todas locales, sin red):
  - KeyStore (~/.nexum/api_keys.json) — providers del Catálogo.
   - XDG data `opencode/auth.json` — familia OpenCode (Zen/Go).
   - CLIPROXYAPI_API_KEY del entorno — providers puenteados (Claude/Codex/Gemini)
     via CLIProxyAPI en 127.0.0.1:8317.
  - Ollama local (sin auth real).

Seguridad: la key sale SOLO por stdout (pipe capturado por el TUI, jamás
logueado). El provider_id viaja por argv porque no es secreto.
Stdlib only.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any, Mapping

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from nexum_providers.catalog_providers import get_catalog_entry
from nexum_providers.key_store import KeyStore

OPENCODE_AUTH_JSON = "XDG_DATA_HOME/opencode/auth.json"

# Espejo de provider_registry.auth_json_keys (catálogo productivo):
# qué entradas de auth.json corresponden a cada familia OpenCode.
#
# Verificado empíricamente (2026-07-05, chat real contra opencode.ai):
#   - /models es PÚBLICO (200 sin auth) — no sirve para validar credenciales.
#   - La key api de `opencode-go` autentica el chat tanto en zen/go/v1 como
#     en zen/v1 (misma cuenta). El token OAuth de la entrada "openai" NO
#     sirve para Zen (401 Invalid API key) — es una credencial de OpenAI,
#     no de Zen; por eso no está en la lista de candidatos.
OPENCODE_FAMILY: dict[str, dict[str, Any]] = {
    "opencode": {
        # Solo una entrada "opencode" real — nunca credenciales de terceros
        # (la entrada "openai" es un token de OpenAI y da 401 en opencode.ai).
        "display_name": "OpenCode",
        "keys": ("opencode",),
        "base_url": "https://opencode.ai/zen/v1",
    },
    "opencode_zen": {
        "display_name": "OpenCode Zen",
        "keys": ("opencode-zen", "opencode_zen", "zen", "opencode", "opencode-go"),
        "base_url": "https://opencode.ai/zen/v1",
    },
    "opencode_go": {
        "display_name": "OpenCode Go",
        "keys": ("opencode-go", "opencode_go", "go"),
        "base_url": "https://opencode.ai/zen/go/v1",
    },
}

# Providers puenteados por CLIProxyAPI (endpoint OpenAI-compatible local).
BRIDGED = {
    "claude_code": "Claude (puente CLIProxyAPI)",
    "codex_cli": "Codex / OpenAI (puente CLIProxyAPI)",
    "gemini_cli": "Gemini (puente CLIProxyAPI)",
}
BRIDGE_BASE_URL = "http://127.0.0.1:8317/v1"
# Config del puente: misma ruta que declara bridge_supervisor.CONFIG_PATH.
BRIDGE_CONFIG_PATH = Path("~/.cli-proxy-api/config.yaml")


def _ok(pid: str, name: str, base_url: str, api_key: str, protocol: str) -> dict:
    return {
        "ok": True,
        "provider_id": pid,
        "display_name": name,
        "base_url": base_url,
        "api_key": api_key,
        "protocol": protocol,
    }


def _err(error: str, message: str) -> dict:
    return {"ok": False, "error": error, "message": message}


def _xdg_data_home(env: Mapping[str, str]) -> Path:
    configured = env.get("XDG_DATA_HOME", "").strip()
    return Path(configured) if configured else Path.home() / ".local/share"


def _extract_key(entry: dict[str, Any]) -> str | None:
    for field in ("key", "access", "token"):
        v = entry.get(field)
        if isinstance(v, str) and v:
            return v
    return None


def _resolve_opencode(pid: str, auth_path: Path) -> dict:
    fam = OPENCODE_FAMILY[pid]
    try:
        auth = json.loads(auth_path.expanduser().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return _err("NOT_CONFIGURED", f"No se pudo leer {OPENCODE_AUTH_JSON}.")
    if not isinstance(auth, dict):
        return _err("NOT_CONFIGURED", "auth.json no es un objeto JSON.")

    keys = fam["keys"]
    candidates: list[dict[str, Any]] = []
    if keys:
        candidates = [auth[k] for k in keys if isinstance(auth.get(k), dict)]
    else:
        claimed = {k for f in OPENCODE_FAMILY.values() for k in f["keys"]}
        candidates = [
            v for k, v in auth.items() if k not in claimed and isinstance(v, dict)
        ]
    # Preferir entradas type=="api" (keys reales de OpenCode): los tokens
    # oauth de otras cuentas no autentican el chat de Zen/Go.
    candidates.sort(key=lambda e: 0 if e.get("type") == "api" else 1)
    for entry in candidates:
        api_key = _extract_key(entry)
        if api_key:
            return _ok(pid, fam["display_name"], fam["base_url"], api_key, "openai")
    if candidates:
        return _err("NOT_CONFIGURED", f"Entrada de {pid} sin clave utilizable.")
    return _err("NOT_CONFIGURED", f"auth.json sin entrada para {pid}.")


def bridge_api_key_from_config(path: Path | None = None) -> str:
    """Primera entrada de `api-keys:` en la config del puente.

    Fallback cuando CLIPROXYAPI_API_KEY no está en el entorno: el puente ya
    tiene su api-key en disco y el usuario no debería tener que duplicarla en
    una variable. Parser mínimo (stdlib only, sin PyYAML): sólo reconoce el
    bloque `api-keys:` seguido de ítems `- valor`.

    La clave se devuelve al llamador y jamás se imprime ni se persiste.
    """
    config = (path or BRIDGE_CONFIG_PATH).expanduser()
    try:
        lines = config.read_text(encoding="utf-8").splitlines()
    except OSError:
        return ""
    in_block = False
    for raw in lines:
        line = raw.rstrip()
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        stripped = line.strip()
        if not line.startswith((" ", "\t", "-")) and stripped.endswith(":"):
            in_block = stripped[:-1].strip() == "api-keys"
            continue
        if in_block and stripped.startswith("-"):
            value = stripped[1:].strip().strip('"').strip("'")
            if value:
                return value
    return ""


def _resolve_bridge(pid: str, env: Mapping[str, str]) -> dict:
    api_key = env.get("CLIPROXYAPI_API_KEY", "").strip() or bridge_api_key_from_config()
    if not api_key:
        return _err(
            "NOT_CONFIGURED",
            "Sin CLIPROXYAPI_API_KEY en el entorno ni `api-keys` en la config "
            "del puente (~/.cli-proxy-api/config.yaml).",
        )
    return _ok(pid, BRIDGED[pid], BRIDGE_BASE_URL, api_key, "openai")


def resolve(
    provider_id: str,
    *,
    key_store: KeyStore | None = None,
    auth_json_path: Path | None = None,
    env: Mapping[str, str] | None = None,
) -> dict:
    """Resolución pura-inyectable (para tests). Sin red."""
    pid = provider_id.strip()
    norm = pid.replace("-", "_").lower()
    environment = dict(os.environ if env is None else env)

    # Ollama local (auth dummy, endpoint fijo).
    if norm in ("ollama_local", "ollama"):
        return _ok(pid, "Ollama Local", "http://127.0.0.1:11434/v1", "ollama", "openai")

    # KeyStore (providers del Catálogo ya logueados).
    store = key_store or KeyStore()
    if store.has_key(norm):
        raw = store.get_key(norm) or ""
        stored = store.get_stored(norm)
        centry = get_catalog_entry(norm)
        name = centry.display_name if centry else norm
        base_url = stored.base_url if stored else (centry.base_url if centry else "")
        protocol = stored.protocol if stored else "openai"
        if raw and base_url:
            return _ok(norm, name, base_url, raw, protocol)
        return _err("NOT_CONFIGURED", f"Credencial incompleta para '{norm}'.")

    # Familia OpenCode (auth.json).
    if norm in OPENCODE_FAMILY:
        path = auth_json_path or (_xdg_data_home(environment) / "opencode/auth.json")
        return _resolve_opencode(norm, path)

    # MiMo Code (Xiaomi): mismo esquema auth.json pero en su propio data-dir,
    # con la base URL regional del usuario en entry.metadata.base_url.
    if norm == "mimo_code":
        path = auth_json_path or (_xdg_data_home(environment) / "mimocode/auth.json")
        try:
            auth = json.loads(path.expanduser().read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return _err("NOT_CONFIGURED", "No se pudo leer el auth.json de MiMo Code.")
        entry = auth.get("xiaomi") if isinstance(auth, dict) else None
        if not isinstance(entry, dict):
            return _err(
                "NOT_CONFIGURED", "auth.json de MiMo sin entrada 'xiaomi' (¿logueado?)."
            )
        api_key = _extract_key(entry)
        if not api_key:
            return _err(
                "NOT_CONFIGURED", "Entrada xiaomi de MiMo sin clave utilizable."
            )
        metadata = entry.get("metadata")
        base_url = (
            metadata.get("base_url")
            if isinstance(metadata, dict) and isinstance(metadata.get("base_url"), str)
            else "https://api.xiaomimimo.com/v1"
        )
        return _ok(norm, "MiMo Code (Xiaomi)", base_url, api_key, "openai")

    # Puenteados vía CLIProxyAPI.
    if norm in BRIDGED:
        return _resolve_bridge(norm, environment)

    return _err("UNSUPPORTED", f"No sé resolver credenciales para '{provider_id}'.")


def main(argv: list[str] | None = None) -> int:
    args = argv if argv is not None else sys.argv[1:]
    if len(args) != 1 or not args[0].strip():
        print(json.dumps(_err("UNSUPPORTED", "Uso: provider_resolve.py <provider_id>")))
        return 1
    outcome = resolve(args[0])
    print(json.dumps(outcome, ensure_ascii=False))
    return 0 if outcome.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main())
