"""nexum_hormiguero_sidecar — puente HTTP loopback entre la TUI Rust y el
Hormiguero Python (Sprint 0 del pasillo, ADR-NEXUM-HORMIGUERO-VOICE-CAPABILITY-FOUNDATION D1).

Stdlib only (regla de la capa Python de esta CLI, igual que nexum_providers).
Solo loopback. Token 0600 obligatorio. Nunca loggea contenido de mensajes.

Diseño Cartero V3: clasificación determinística primero; el residuo puede
usar qwen3:0.6b vía Ollama local (best-effort, con sub-budget). Si nada
alcanza, la decisión es SIEMPRE escalar (fail-safe: nunca adivinar).
"""

__version__ = "0.1.0"
