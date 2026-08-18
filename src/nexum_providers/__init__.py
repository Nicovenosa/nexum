"""nexum_providers — módulo productivo de proveedores (ADR-044, cierre).

Consolida la infraestructura productiva de providers:

  - catalog_providers: registry de proveedores pre-configurados (Catálogo)
  - key_store: almacenamiento seguro de API keys (600, fingerprint enmascarado)
  - probe_validator: validación en vivo de keys (OpenAI + Anthropic)
  - provider_login: CLI de login de un paso (probe + store + upsert catálogo)

Seguridad: ningún módulo imprime ni loguea valores de keys/tokens.
Stdlib only. Sin imports del runtime de Nexum.
"""

from __future__ import annotations
