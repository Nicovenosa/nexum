"""Clasificador del sidecar — diseño Cartero V3: determinístico primero,
LLM local (qwen3:0.6b vía Ollama) solo para el residuo y solo best-effort.

Regla fail-safe: si nada clasifica con confianza, la decisión es escalar
(needs_paid_ai). El Hormiguero nunca adivina ni responde de más.

PRIVACIDAD: este módulo no loggea el texto del usuario. Nunca.
"""

from __future__ import annotations

import json
import os
import re
import threading
import time
import urllib.request
from dataclasses import dataclass

# Modelo interno del Hormiguero. NUNCA se expone como nombre público:
# /status devuelve "nexum-local" salvo modo debug explícito.
_INTERNAL_MODEL = "qwen3:0.6b"
_PUBLIC_MODEL_NAME = "nexum-local"
_OLLAMA_URL = os.environ.get("NEXUM_OLLAMA_URL", "http://127.0.0.1:11434")

# ── Capa determinística ─────────────────────────────────────────────────
# Patrones es/en para los triviales del spec. Texto corto + match = local.

_TRIVIAL_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    (
        # Saludo PURO: saludo + cortesía opcional, NUNCA "saludo + tarea".
        # (Q-1 fix 0.1.2: el .{0,40} viejo tragaba tareas — "hola, revisá
        #  vulnerabilidades" caía como trivial → violación del fail-safe.)
        "smalltalk",
        re.compile(
            r"^(hola|buenas(\s+(tardes|noches))?|buen\s?d[ií]a|hey|hi|hello|holis"
            r"|qu[eé]\s+tal|qu[eé]\s+onda)(\s+nexum)?[\s,.!¡?]*"
            r"((c[oó]mo\s+(va|and[aá]s|est[aá]s|te\s+va|vas|anda|le\s+va)"
            r"|qu[eé]\s+tal|todo\s+bien|qu[eé]\s+onda|there"
            r"|how\s+(are\s+you|is\s+it\s+going|are\s+things)|what'?s\s+up)[\s,.!¡?]*)?$",
            re.I,
        ),
    ),
    (
        "status",
        re.compile(
            r"(est[aá]s(\s*(ah[ií]|activo|vivo|bien|escuchando))?\s*\??$"
            r"|escuch[aá]s|are you (there|listening|alive|ok)"
            r"|me\s+(o[ií]s|escuch[aá]s))",
            re.I,
        ),
    ),
    ("status", re.compile(r"^(nexum[,\s]+)?(est[aá]s|estas)\s*\??\s*$", re.I)),
    (
        "command",
        re.compile(
            r"^(par[aá]|stop|cancel[aá]?|cancel|repet[ií]|repeat"
            r"|le[eé]\s+el\s+[uú]ltimo\s+mensaje"
            r"|copi[aá]\s+la\s+respuesta"
            r"|mostrame\s+(ayuda|proveedores|modelos)"
            r"|abr[ií]\s+ayuda|estado\s+del\s+sistema"
            r"|no\s+hagas\s+nada.{0,30})\s*\.?\s*$",
            re.I,
        ),
    ),
    (
        # Agradecimiento PURO, no "gracias + tarea" (Q-1 fix 0.1.2).
        "smalltalk",
        re.compile(
            r"^(gracias(\s+(totales|che|nexum|mil))?|muchas\s+gracias|thanks"
            r"|thank\s+you|genial|ok(\s+gracias)?|dale|listo|perfecto"
            r"|buen[ií]simo|joya|de\s+diez|de\s+una)\s*[?!.]*$",
            re.I,
        ),
    ),
]

# Señales de tarea compleja (largo, código, verbos de trabajo profundo).
_COMPLEX_HINTS = re.compile(
    r"(analiz|refactor|dise[ñn]|arquitect|debug|implement|investig|optimiz"
    r"|escrib[ií]|gener[aá]|explic[aá]\s+en\s+detalle|compar[aá]|audit"
    r"|```|def |fn |class |import |trade-?off)",
    re.I,
)

_LOCAL_ANSWERS_ES: dict[str, str] = {
    "smalltalk": "¡Hola! Acá estoy, escuchando. ¿En qué te ayudo?",
    "status": "Sí, estoy activo. El runtime local de Nexum está funcionando y listo.",
    "command": "Entendido. Ese es un comando de la interfaz: usá el atajo o comando correspondiente (por ejemplo /help para ayuda, Ctrl+C para copiar la última respuesta).",
}
_LOCAL_ANSWERS_EN: dict[str, str] = {
    "smalltalk": "Hi! I'm here and listening. How can I help?",
    "status": "Yes, I'm up. Nexum's local runtime is running and ready.",
    "command": "Got it. That's an interface command: use the matching shortcut or slash command (e.g. /help, or Ctrl+C to copy the last answer).",
}


@dataclass
class Classification:
    intent: str  # smalltalk|status|command|complex_task|unknown
    complexity: str  # trivial|medium|complex (valores realmente emitidos)
    can_handle_local: bool
    should_escalate: bool
    confidence: float
    reason_public: str  # breve, seguro, sin reasoning interno
    engine: str  # "deterministic" | "local_llm" | "fail_safe"


def classify(text: str, max_latency_ms: int = 350) -> Classification:
    stripped = text.strip()

    # 1. Determinístico (cubre los triviales del spec en <1ms).
    if len(stripped) <= 120 and not _COMPLEX_HINTS.search(stripped):
        for intent, pattern in _TRIVIAL_PATTERNS:
            if pattern.search(stripped):
                return Classification(
                    intent=intent,
                    complexity="trivial",
                    can_handle_local=True,
                    should_escalate=False,
                    confidence=0.95,
                    reason_public="pedido simple resuelto localmente",
                    engine="deterministic",
                )

    # 2. Señales claras de complejidad → escalar sin gastar LLM local.
    if _COMPLEX_HINTS.search(stripped) or len(stripped) > 200:
        return Classification(
            intent="complex_task",
            complexity="complex",
            can_handle_local=False,
            should_escalate=True,
            confidence=0.9,
            reason_public="tarea compleja: requiere el modelo principal",
            engine="deterministic",
        )

    # 3. Residuo → LLM local best-effort dentro del sub-budget.
    #    F-1 (evidencia): la etapa Qwen residual (qwen3:0.6b) mide ~352ms y en
    #    la práctica agota su budget y escala igual (valor marginal). Queda
    #    OPT-IN con NEXUM_HORMIGUERO_RESIDUAL_LLM=1; por defecto se salta y va
    #    directo al fail-safe determinístico (0 latencia de LLM local).
    import os

    if os.environ.get("NEXUM_HORMIGUERO_RESIDUAL_LLM") == "1":
        llm = _classify_residual_llm(stripped, max_latency_ms)
        if llm is not None:
            return llm

    # 4. Fail-safe: no adivinar. Escalar.
    return Classification(
        intent="unknown",
        complexity="medium",
        can_handle_local=False,
        should_escalate=True,
        confidence=0.4,
        reason_public="sin clasificación local confiable: se deriva al modelo principal",
        engine="fail_safe",
    )


def local_answer(intent: str, locale: str) -> str | None:
    table = _LOCAL_ANSWERS_ES if locale.startswith("es") else _LOCAL_ANSWERS_EN
    return table.get(intent)


def _classify_residual_llm(text: str, budget_ms: int) -> Classification | None:
    """Residuo vía qwen3:0.6b (Ollama, thinking OFF — regla del repo).

    Best-effort: cualquier fallo/timeout devuelve None (→ fail-safe).
    Solo clasifica; NUNCA genera la respuesta al usuario.
    """
    if budget_ms < 150:
        return None
    payload = {
        "model": _INTERNAL_MODEL,
        "stream": False,
        "think": False,
        "options": {"num_predict": 8, "temperature": 0.0},
        "messages": [
            {
                "role": "system",
                "content": (
                    "Clasificá el mensaje del usuario en UNA palabra exacta: "
                    "smalltalk, status, command, complex_task o unknown. "
                    "Solo la palabra."
                ),
            },
            {"role": "user", "content": text[:400]},
        ],
    }
    req = urllib.request.Request(
        f"{_OLLAMA_URL}/api/chat",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    start = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=budget_ms / 1000.0) as resp:
            body = json.loads(resp.read().decode())
        word = (body.get("message", {}).get("content") or "").strip().lower()
    except Exception:
        return None
    elapsed_ms = (time.monotonic() - start) * 1000
    if elapsed_ms > budget_ms:
        return None
    if word in ("smalltalk", "status", "command"):
        return Classification(
            intent=word,
            complexity="trivial",
            can_handle_local=True,
            should_escalate=False,
            confidence=0.75,
            reason_public="pedido simple resuelto localmente",
            engine="local_llm",
        )
    if word == "complex_task":
        return Classification(
            intent="complex_task",
            complexity="complex",
            can_handle_local=False,
            should_escalate=True,
            confidence=0.7,
            reason_public="tarea compleja: requiere el modelo principal",
            engine="local_llm",
        )
    return None


# Cache TTL del probe a Ollama (OMEGA Fase 9, cierra H-3): /status NO debe
# golpear Ollama por consulta. Resultado (positivo o negativo) cacheado 30s;
# _OLLAMA_URL es fijo por proceso, así que no hay invalidación por config.
_MODEL_PROBE_TTL_S = 30.0
_model_probe_lock = threading.Lock()
_model_probe_cache: tuple[float, bool] | None = None


def model_available() -> bool:
    """¿Ollama responde y tiene el modelo interno? (cacheado con TTL)."""
    global _model_probe_cache
    now = time.monotonic()
    with _model_probe_lock:
        if _model_probe_cache is not None:
            at, ok = _model_probe_cache
            if now - at < _MODEL_PROBE_TTL_S:
                return ok
    ok = _probe_model_uncached()
    with _model_probe_lock:
        _model_probe_cache = (time.monotonic(), ok)
    return ok


def _probe_model_uncached() -> bool:
    try:
        with urllib.request.urlopen(f"{_OLLAMA_URL}/api/tags", timeout=1.0) as resp:
            data = json.loads(resp.read().decode())
        return any(m.get("name") == _INTERNAL_MODEL for m in data.get("models", []))
    except Exception:
        return False


def public_model_name(debug: bool) -> str:
    return _INTERNAL_MODEL if debug else _PUBLIC_MODEL_NAME
