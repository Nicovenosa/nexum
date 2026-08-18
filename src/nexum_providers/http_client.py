"""Cliente HTTP compartido para todos los probes de provider.

Existe por una razón concreta y medida: `opencode.ai` responde **403 a cualquier
request con el User-Agent por defecto de Python**.

    curl (UA propio)               -> 200
    curl -A "Python-urllib/3.12"   -> 403
    urllib (UA por defecto)        -> 403
    urllib con UA declarado        -> 200

Un probe que no declara UA concluye "credencial inválida" cuando el problema es
el filtro del proveedor. Todo probe de Nexum pasa por acá para que ese error no
se pueda repetir en un call site nuevo.

Seguridad: la credencial viaja sólo en el header Authorization, nunca en la URL
ni en logs. Los errores no incluyen el cuerpo de la request.

Stdlib only.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Mapping

# Versión del producto en el UA: algunos proveedores filtran por agente
# desconocido, y un UA identificable es además lo correcto para un cliente.
NEXUM_USER_AGENT = "nexum/0.1.4 (+https://github.com/Nicovenosa/nexum)"

DEFAULT_TIMEOUT_SECS = 8.0


@dataclass(frozen=True)
class HttpResult:
    """Resultado de un probe. `body` es texto crudo; nunca contiene la credencial."""

    status: int | None
    body: str
    error: str | None = None

    @property
    def ok(self) -> bool:
        return self.status == 200

    def json(self) -> Any | None:
        try:
            return json.loads(self.body)
        except ValueError:
            return None


def default_headers(extra: Mapping[str, str] | None = None) -> dict[str, str]:
    """Headers base de todo request de Nexum. El UA nunca es opcional."""
    headers = {
        "User-Agent": NEXUM_USER_AGENT,
        "Accept": "application/json",
    }
    if extra:
        headers.update(extra)
    return headers


def request(
    url: str,
    *,
    api_key: str | None = None,
    method: str = "GET",
    payload: Mapping[str, Any] | None = None,
    timeout: float = DEFAULT_TIMEOUT_SECS,
) -> HttpResult:
    """Request con UA declarado. Nunca lanza: los fallos vuelven en HttpResult.

    Un cuerpo de error se conserva porque muchos proveedores distinguen ahí
    entre "falta credencial", "credencial inválida" y "credencial válida sin
    saldo" — información que vale más que el código de estado solo.
    """
    if not url.startswith(("http://", "https://")):
        return HttpResult(None, "", "esquema de URL no soportado")

    headers = default_headers({"Content-Type": "application/json"} if payload else None)
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"

    data = json.dumps(payload).encode("utf-8") if payload is not None else None
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:  # noqa: S310
            return HttpResult(
                response.status, response.read().decode("utf-8", "replace")
            )
    except urllib.error.HTTPError as exc:
        try:
            body = exc.read().decode("utf-8", "replace")
        except OSError:
            body = ""
        return HttpResult(exc.code, body)
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        return HttpResult(None, "", type(exc).__name__)
