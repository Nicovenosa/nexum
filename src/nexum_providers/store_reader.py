"""Lectura de credenciales desde los almacenes DECLARADOS en el registry.

Reemplaza las rutas fijas que vivían dentro de la lógica de resolución. Esa
forma de escribirlo produjo tres bugs del mismo tipo, encontrados en el
relevamiento del Sprint 2:

  * a `opencode` se le miraba `auth.json`, pero su credencial estaba en
    `account.json`;
  * la api-key del puente estaba en su config y sólo se buscaba en el entorno;
  * `OPENCODE_GO_API_KEY` estaba exportada y nadie la leía.

Con los almacenes declarados, agregar un lugar donde buscar es editar el
registry, no la lógica.

El ORDEN de `credential_stores` es la precedencia y espeja el de la CLI dueña.
Se devuelve además QUÉ almacén ganó, para que el catálogo pueda mostrarlo: en
esta máquina conviven cuatro valores distintos de "la key de OpenCode" y saber
cuál se usó es la diferencia entre diagnosticar y adivinar.

Seguridad: el secreto se devuelve al llamador y jamás se loguea. `store_path`
sale con el home colapsado a `~`, apto para mostrar en pantalla.

Stdlib only.
"""

from __future__ import annotations

import json
import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

from nexum_providers.catalog_gen.provider_registry import CredentialStore, StoreKind


@dataclass(frozen=True)
class ResolvedCredential:
    provider_id: str
    secret: str
    store_kind: str
    store_path: str
    store_legacy: bool
    entry: str
    base_url: str | None = None

    def safe_summary(self) -> dict[str, Any]:
        """Descripción publicable: identifica el almacén, nunca el secreto."""
        return {
            "credential_store": self.store_path,
            "credential_store_kind": self.store_kind,
            "credential_store_legacy": self.store_legacy,
            "credential_entry": self.entry,
        }


def _expand(path: str, env: Mapping[str, str]) -> Path:
    home = Path(env.get("HOME") or Path.home())
    data = env.get("XDG_DATA_HOME") or str(home / ".local/share")
    config = env.get("XDG_CONFIG_HOME") or str(home / ".config")
    expanded = (
        path.replace("$XDG_DATA_HOME", data)
        .replace("$XDG_CONFIG_HOME", config)
        .replace("~", str(home), 1)
    )
    return Path(expanded)


def _display(path: Path, env: Mapping[str, str]) -> str:
    home = str(env.get("HOME") or Path.home())
    text = str(path)
    return "~" + text[len(home) :] if text.startswith(home) else text


def _load_json(path: Path) -> Any | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def _first_field(entry: Mapping[str, Any], fields: tuple[str, ...]) -> str | None:
    for name in fields:
        value = entry.get(name)
        if isinstance(value, str) and value:
            return value
    return None


def _base_url_of(entry: Mapping[str, Any]) -> str | None:
    """La base URL por usuario puede venir en metadata (caso MiMo regional)."""
    metadata = entry.get("metadata")
    if isinstance(metadata, dict):
        url = metadata.get("base_url")
        if isinstance(url, str) and url:
            return url
    url = entry.get("base_url")
    return url if isinstance(url, str) and url else None


def _read_json_map(
    store: CredentialStore, path: Path
) -> tuple[str, str, str | None] | None:
    doc = _load_json(path)
    if not isinstance(doc, dict):
        return None
    for name in store.entries:
        entry = doc.get(name)
        if isinstance(entry, dict):
            secret = _first_field(entry, store.fields)
            if secret:
                return secret, name, _base_url_of(entry)
    return None


def _read_json_accounts(
    store: CredentialStore, path: Path
) -> tuple[str, str, str | None] | None:
    doc = _load_json(path)
    if not isinstance(doc, dict):
        return None
    accounts = doc.get("accounts")
    if not isinstance(accounts, dict):
        return None
    wanted = {name.lower() for name in store.entries}
    for account in accounts.values():
        if not isinstance(account, dict):
            continue
        service = account.get("serviceID")
        if not isinstance(service, str) or service.lower() not in wanted:
            continue
        credential = account.get("credential")
        if isinstance(credential, dict):
            secret = _first_field(credential, store.fields)
            if secret:
                return secret, service, _base_url_of(credential)
    return None


def _read_env(
    store: CredentialStore, env: Mapping[str, str]
) -> tuple[str, str, str | None] | None:
    for name in store.env_vars:
        value = (env.get(name) or "").strip()
        if value:
            return value, name, None
    return None


def _read_env_file(
    store: CredentialStore, path: Path
) -> tuple[str, str, str | None] | None:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return None
    wanted = set(store.env_vars)
    for raw in lines:
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        match = re.match(r"^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$", line)
        if not match:
            continue
        name, value = match.group(1), match.group(2).strip().strip('"').strip("'")
        if name in wanted and value:
            return value, name, None
    return None


def _read_yaml_list(
    store: CredentialStore, path: Path
) -> tuple[str, str, str | None] | None:
    """Parser mínimo para `clave:` seguido de ítems `- valor` (sin PyYAML)."""
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return None
    in_block = False
    for raw in lines:
        line = raw.rstrip()
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if not line.startswith((" ", "\t", "-")) and stripped.endswith(":"):
            in_block = stripped[:-1].strip() == (store.yaml_key or "")
            continue
        if in_block and stripped.startswith("-"):
            value = stripped[1:].strip().strip('"').strip("'")
            if value:
                return value, store.yaml_key or "", None
    return None


def read_store(
    store: CredentialStore, env: Mapping[str, str]
) -> tuple[str, str, str | None] | None:
    """(secreto, entrada, base_url) del almacén, o None si no hay nada."""
    if store.kind == StoreKind.ENV:
        return _read_env(store, env)
    if store.path is None:
        return None
    path = _expand(store.path, env)
    readers = {
        StoreKind.JSON_MAP: _read_json_map,
        StoreKind.JSON_NESTED: _read_json_map,  # misma forma anidada
        StoreKind.JSON_ACCOUNTS: _read_json_accounts,
        StoreKind.ENV_FILE: _read_env_file,
        StoreKind.YAML_LIST: _read_yaml_list,
    }
    reader = readers.get(store.kind)
    return reader(store, path) if reader else None


def resolve_from_stores(
    definition: Any, env: Mapping[str, str] | None = None
) -> ResolvedCredential | None:
    """Primer almacén de la lista que resuelva, gana. `None` si ninguno tiene."""
    environment = dict(os.environ if env is None else env)
    for store in getattr(definition, "credential_stores", ()) or ():
        found = read_store(store, environment)
        if found is None:
            continue
        secret, entry, base_url = found
        path = _expand(store.path, environment) if store.path else None
        return ResolvedCredential(
            provider_id=definition.provider_id,
            secret=secret,
            store_kind=store.kind,
            store_path=_display(path, environment) if path else f"env:{entry}",
            store_legacy=store.legacy,
            entry=entry,
            base_url=base_url or definition.base_url_hint,
        )
    return None
