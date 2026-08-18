"""Verificación de credenciales con estados honestos y sin ráfagas de requests.

Estados (B.1). Los tres primeros son los pedidos; los dos últimos los impuso la
evidencia y omitirlos sería mentir en la UI:

  verified            la credencial fue probada positivamente y el provider sirve
  free_access         funciona SIN credencial (tier gratuito), probado con una
                      llamada real que devolvió contenido
  present_unverified  hay credencial pero no se pudo probar — NUNCA se muestra
                      como usable sin marca
  verified_no_credit  la credencial es VÁLIDA pero la cuenta no tiene saldo. Es
                      distinto de inválida: el proveedor sólo responde
                      "insufficient balance" a una credencial que reconoció.
                      Medido en OpenCode Zen/Go el 2026-07-25.
  invalid             la credencial fue rechazada explícitamente

Coste: cuando el proveedor tiene un endpoint que devuelve 401 sin credencial y
200 con ella (caso MiMo `/models`), verificar **no gasta tokens**. Cuando no lo
hay, se cae a una completion mínima (`max_tokens` 1) y el resultado se cachea
por fingerprint con TTL, así que un refresh no dispara una ráfaga.

Stdlib only.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any

from nexum_providers import http_client
from nexum_providers.verification_cache import VerificationCache

STATE_VERIFIED = "verified"
STATE_FREE_ACCESS = "free_access"
STATE_PRESENT_UNVERIFIED = "present_unverified"
STATE_VERIFIED_NO_CREDIT = "verified_no_credit"
STATE_INVALID = "invalid"
# Cuota mensual agotada en una suscripción ACTIVA. Distinto de "sin saldo":
# esto se destraba solo en una fecha conocida. Medido en OpenCode Go el
# 2026-07-26: 429 "Monthly usage limit reached. Resets in 8 days."
STATE_QUOTA_EXHAUSTED = "quota_exhausted"

# Estados en los que el provider puede seleccionarse en /modelo.
USABLE_STATES = frozenset({STATE_VERIFIED, STATE_FREE_ACCESS})

# Veredictos que dicen algo definitivo sobre la credencial y por lo tanto se
# pueden cachear. `present_unverified` queda afuera a propósito: significa "no
# sé", y guardarlo convertiría un 429 pasajero en seis horas de incertidumbre.
CONCLUSIVE_STATES = frozenset(
    {
        STATE_VERIFIED,
        STATE_FREE_ACCESS,
        STATE_VERIFIED_NO_CREDIT,
        STATE_INVALID,
        STATE_QUOTA_EXHAUSTED,
    }
)

# "Monthly usage limit reached. Resets in 8 days." — suscripción viva.
_QUOTA = re.compile(
    r"monthly\s+usage\s+limit|usage\s+limit\s+reached|quota\s+reset", re.I
)
_RESETS_IN = re.compile(r"resets?\s+in\s+(\d+)\s+day", re.I)


def _quota_detail(body: str) -> str:
    """Traduce el 429 a un estado con FECHA: cuándo se destraba solo."""
    match = _RESETS_IN.search(body)
    if not match:
        return "cuota mensual agotada; el proveedor no informó la fecha de renovación"
    import datetime as _dt

    dias = int(match.group(1))
    fecha = _dt.date.today() + _dt.timedelta(days=dias)
    return f"cuota mensual agotada · se renueva ~{fecha:%Y-%m-%d} ({dias} días)"


# El cuerpo del 401 distingue tres situaciones muy distintas. Verificado contra
# opencode.ai: "Missing API key." / "Invalid API key." / "Insufficient balance".
_NO_CREDIT = re.compile(
    r"insufficient\s+balance|no\s+credit|quota\s+exceeded|billing", re.I
)
_INVALID = re.compile(
    r"invalid\s+(api\s+)?key|unauthorized|forbidden|invalid_key", re.I
)
_MISSING = re.compile(
    r"missing\s+(api\s+)?key|no\s+api\s+key|api\s+key\s+required", re.I
)

# Prompt de verificación: el más corto posible que igual ejercite el camino real.
_PROBE_PAYLOAD_TEMPLATE = {
    "messages": [{"role": "user", "content": "hi"}],
    "max_tokens": 1,
}


@dataclass(frozen=True)
class VerificationResult:
    state: str
    detail: str
    cached: bool = False

    @property
    def usable(self) -> bool:
        return self.state in USABLE_STATES


# ─── Clasificación de respuestas HTTP ────────────────────────────────────────
#
# Esto vive en el código y no en la cabeza de nadie. El 2026-07-26 un 429 de
# `laguna-s-2.1-free` casi lo saca del conjunto de modelos gratuitos: un rate
# limit del proveedor se había leído como "pide credencial". Al reintentar
# respondió 200 sin credencial. La distinción es la diferencia entre recortar
# una funcionalidad real y esperar tres segundos.

VERDICT_AUTHORIZATION = "authorization"  # 401/403: el proveedor JUZGÓ la credencial
VERDICT_TRANSIENT = "transient"  # 429: pasajero, se reintenta
VERDICT_INCONCLUSIVE = "inconclusive"  # 5xx / red: no dice nada de la credencial
VERDICT_OK = "ok"

# Reintentos ante transitorio. Corto a propósito: si el proveedor está
# saturado, insistir empeora las cosas.
TRANSIENT_RETRIES = 2
TRANSIENT_BACKOFF_SECS = 3.0


def classify_http(status: int | None) -> str:
    """Qué SIGNIFICA un código, antes de mirar el cuerpo.

    Un 429 nunca es un veredicto sobre la credencial, y un 5xx tampoco.
    Tratarlos como rechazo produce recortes silenciosos de funcionalidad.
    """
    if status is None:
        return VERDICT_INCONCLUSIVE
    if status == 200:
        return VERDICT_OK
    if status in (401, 403):
        return VERDICT_AUTHORIZATION
    if status == 429:
        return VERDICT_TRANSIENT
    if 500 <= status < 600:
        return VERDICT_INCONCLUSIVE
    return VERDICT_INCONCLUSIVE


def is_retryable(status: int | None) -> bool:
    return classify_http(status) == VERDICT_TRANSIENT


def _classify_error_body(status: int | None, body: str) -> tuple[str, str]:
    # Un transitorio NO se clasifica como veredicto de credencial. La cuota
    # mensual es la excepción: viaja como 429 pero SÍ es información firme
    # sobre el estado de la cuenta, y trae la fecha de renovación.
    if classify_http(status) == VERDICT_TRANSIENT and not _QUOTA.search(body):
        return STATE_PRESENT_UNVERIFIED, (
            f"respuesta transitoria del proveedor (HTTP {status}); no dice nada "
            "sobre la credencial"
        )
    # La cuota va PRIMERO: una suscripción activa con el mes agotado no es una
    # cuenta sin saldo, y confundirlas manda a "cargar plata" a alguien que
    # solo tiene que esperar.
    if _QUOTA.search(body):
        return STATE_QUOTA_EXHAUSTED, _quota_detail(body)
    if _NO_CREDIT.search(body):
        return STATE_VERIFIED_NO_CREDIT, "credencial válida, cuenta sin saldo"
    if _MISSING.search(body):
        return STATE_PRESENT_UNVERIFIED, "el proveedor no recibió la credencial"
    if _INVALID.search(body) or status in (401, 403):
        return STATE_INVALID, f"credencial rechazada (HTTP {status})"
    return STATE_PRESENT_UNVERIFIED, f"respuesta no concluyente (HTTP {status})"


def verify_models_endpoint(
    url: str, api_key: str, timeout: float = 8.0
) -> VerificationResult:
    """Para endpoints que exigen credencial para listar modelos. No gasta tokens."""
    anon = http_client.request(url, timeout=timeout)
    authed = http_client.request(url, api_key=api_key, timeout=timeout)

    if authed.ok and not anon.ok:
        # El endpoint distingue: 401 sin key, 200 con key. Prueba positiva limpia.
        return VerificationResult(STATE_VERIFIED, "listado de modelos autenticado")
    if authed.ok and anon.ok:
        # Público: un 200 no prueba nada sobre la credencial (caso OpenCode).
        return VerificationResult(
            STATE_PRESENT_UNVERIFIED,
            "el endpoint de modelos es público: no valida la credencial",
        )
    if authed.status is None:
        return VerificationResult(STATE_PRESENT_UNVERIFIED, f"red: {authed.error}")
    state, detail = _classify_error_body(authed.status, authed.body)
    return VerificationResult(state, detail)


def verify_completion(
    url: str, api_key: str | None, model: str, timeout: float = 30.0
) -> VerificationResult:
    """Completion mínima (`max_tokens` 1). Último recurso; el costo es de 1 token."""
    payload = dict(_PROBE_PAYLOAD_TEMPLATE, model=model)
    result = http_client.request(
        url, api_key=api_key, method="POST", payload=payload, timeout=timeout
    )
    if result.ok:
        if api_key is None:
            return VerificationResult(STATE_FREE_ACCESS, "responde sin credencial")
        return VerificationResult(STATE_VERIFIED, "completion mínima OK")
    if result.status is None:
        return VerificationResult(STATE_PRESENT_UNVERIFIED, f"red: {result.error}")
    state, detail = _classify_error_body(result.status, result.body)
    return VerificationResult(state, detail)


def verify_free_tier(
    url: str, free_model: str, timeout: float = 60.0
) -> VerificationResult:
    """¿El provider sirve este modelo SIN credencial?

    Es la única señal honesta de tier libre: que el endpoint exista no prueba
    nada. Verificado el 2026-07-25 — `opencode.ai/zen/v1/chat/completions` con
    un modelo `-free` y sin Authorization devuelve 200.

    No se compara el contenido: varios de estos modelos gastan el presupuesto
    de tokens en razonamiento y devuelven texto vacío. Un 200 con `choices` es
    la prueba de que el camino funciona sin credencial.
    """
    if not url:
        return VerificationResult(STATE_PRESENT_UNVERIFIED, "sin endpoint declarado")
    # Reintento ante transitorio: un 429 del proveedor no es un veredicto sobre
    # el acceso. Sin esto, un rate limit pasajero recorta un modelo real del
    # conjunto de gratuitos.
    import time as _time

    for intento in range(TRANSIENT_RETRIES + 1):
        result = http_client.request(
            url,
            method="POST",
            payload=dict(_PROBE_PAYLOAD_TEMPLATE, model=free_model),
            timeout=timeout,
        )
        if not is_retryable(result.status) or intento == TRANSIENT_RETRIES:
            break
        _time.sleep(TRANSIENT_BACKOFF_SECS)
    if result.ok and isinstance((result.json() or {}).get("choices"), list):
        return VerificationResult(
            STATE_FREE_ACCESS,
            f"tier libre verificado con «{free_model}» (sin credencial)",
        )
    if result.status is None:
        return VerificationResult(STATE_PRESENT_UNVERIFIED, f"red: {result.error}")
    state, detail = _classify_error_body(result.status, result.body)
    return VerificationResult(state, f"tier libre no disponible: {detail}")


def verify_credential(
    definition: Any,
    resolved: Any,
    *,
    cache: VerificationCache | None = None,
    probe_model: str | None = None,
    timeout: float = 8.0,
) -> VerificationResult:
    """Verifica una credencial resolviendo primero por cache.

    `resolved` es un `store_reader.ResolvedCredential`. El cache se indexa por
    fingerprint, así que rotar la credencial invalida la entrada sola.
    """
    store = cache or VerificationCache()
    hit = store.get(definition.provider_id, resolved.secret)
    if hit is not None:
        return VerificationResult(hit.state, hit.detail, cached=True)

    endpoint = definition.verify_endpoint
    if not endpoint:
        result = VerificationResult(
            STATE_PRESENT_UNVERIFIED, "el provider no declara endpoint de verificación"
        )
    else:
        url = endpoint.replace("{base_url}", (resolved.base_url or "").rstrip("/"))
        if url.endswith("/models"):
            result = verify_models_endpoint(url, resolved.secret, timeout=timeout)
        elif probe_model:
            result = verify_completion(
                url, resolved.secret, probe_model, timeout=timeout
            )
        else:
            result = VerificationResult(
                STATE_PRESENT_UNVERIFIED, "falta un modelo con el cual probar"
            )

    # Sólo se cachea un veredicto CONCLUYENTE. Un 429, un timeout o un endpoint
    # público dejan `present_unverified`, que no dice nada sobre la credencial:
    # cachearlo congelaría un fallo transitorio durante todo el TTL y el
    # provider quedaría "sin verificar" horas después de que el problema pasó.
    if result.state in CONCLUSIVE_STATES:
        store.put(
            definition.provider_id,
            resolved.secret,
            result.state,
            result.detail,
            resolved.store_path,
        )
        store.save()
    return result
