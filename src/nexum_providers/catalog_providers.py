"""Nexum Catalog Providers — pre-configured providers for /provedor Catálogo section.

Sprint 4: providers that accept a pasted API key with one-step login flow.
Each entry carries: provider_id, display_name, base_url (pre-filled),
protocol (anthropic | openai), models_endpoint (for probe), and key_env_hint
(one-line instruction on where to get the key).

Security: this module defines NO secret values — only URLs and display strings.
Stdlib only. No Nexum runtime imports.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class CatalogProviderEntry:
    """A pre-configured provider for the Catálogo section of /provedor."""

    provider_id: str
    display_name: str
    base_url: str
    protocol: str  # "anthropic" | "openai"
    models_endpoint: str  # full URL for probe validation
    key_env_hint: str  # one-line instruction
    static_models: tuple[str, ...] = ()  # fallback models if probe fails
    # True when the user must supply the base URL (region-specific providers,
    # e.g. Xiaomi MiMo Token Plan). base_url then acts only as a placeholder
    # pattern shown in the input field; models_endpoint is derived at login
    # time as {base_url}/models.
    needs_base_url: bool = False


# ─── The catalog ──────────────────────────────────────────────────────────────
#
# URLs verified against official documentation (2026-07-04):
#   Anthropic: https://docs.anthropic.com/en/api/getting-started
#   OpenAI: https://platform.openai.com/docs/api-reference
#   Google Gemini: https://ai.google.dev/gemini-api/docs/openai (OpenAI-compat)
#   GLM Coding Plan: https://docs.z.ai/devpack/quick-start (Sprint 2)
#   DeepSeek: https://api-docs.deepseek.com/
#   Moonshot/Kimi: https://platform.moonshot.ai/docs/api/chat (verified)
#   OpenRouter: https://openrouter.ai/docs/quick-start
#
# Providers NOT included:
#   OpenCode Zen/Go — already auto-detected, catalog is manual fallback only
#
# Xiaomi MiMo (Token Plan) IS included since the cierre sprint, with
# needs_base_url=True: the base URL is region-specific per user dashboard, so
# the login flow asks for it (placeholder pattern) before the API key.

CATALOG_PROVIDERS: dict[str, CatalogProviderEntry] = {
    "anthropic_api_key": CatalogProviderEntry(
        provider_id="anthropic_api_key",
        display_name="Anthropic / Claude",
        base_url="https://api.anthropic.com",
        protocol="anthropic",
        models_endpoint="https://api.anthropic.com/v1/models",
        key_env_hint="console.anthropic.com → API Keys",
        static_models=("claude-opus-4-8", "claude-sonnet-5"),
    ),
    "openai": CatalogProviderEntry(
        provider_id="openai",
        display_name="OpenAI",
        base_url="https://api.openai.com/v1",
        protocol="openai",
        models_endpoint="https://api.openai.com/v1/models",
        key_env_hint="platform.openai.com → API Keys",
        static_models=("gpt-5.4", "gpt-5.4-mini"),
    ),
    "google_api_key": CatalogProviderEntry(
        provider_id="google_api_key",
        display_name="Google / Gemini",
        base_url="https://generativelanguage.googleapis.com/v1beta/openai",
        protocol="openai",
        models_endpoint="https://generativelanguage.googleapis.com/v1beta/openai/models",
        key_env_hint="aistudio.google.com → API Keys",
        static_models=("gemini-2.5-pro", "gemini-2.5-flash"),
    ),
    "glm_coding_plan": CatalogProviderEntry(
        provider_id="glm_coding_plan",
        display_name="Z.ai / GLM Coding Plan",
        base_url="https://api.z.ai/api/coding/paas/v4",
        protocol="openai",
        models_endpoint="https://api.z.ai/api/coding/paas/v4/models",
        key_env_hint="z.ai/manage-apikey/apikey-list",
        # Modelos del GLM Coding Plan (models.dev/providers/zai-coding-plan).
        # glm-5-turbo reemplaza a glm-5.1 en la spec de Nexum: contexto 200k.
        static_models=("glm-5.2", "glm-5-turbo"),
    ),
    "deepseek": CatalogProviderEntry(
        provider_id="deepseek",
        display_name="DeepSeek",
        base_url="https://api.deepseek.com",
        protocol="openai",
        models_endpoint="https://api.deepseek.com/v1/models",
        key_env_hint="platform.deepseek.com → API Keys",
        static_models=("deepseek-v4-pro", "deepseek-v4-flash"),
    ),
    "moonshot": CatalogProviderEntry(
        provider_id="moonshot",
        display_name="Moonshot / Kimi",
        base_url="https://api.moonshot.ai/v1",
        protocol="openai",
        models_endpoint="https://api.moonshot.ai/v1/models",
        key_env_hint="platform.moonshot.ai → API Keys",
        static_models=("kimi-k2.7-code", "kimi-k2.6"),
    ),
    "openrouter": CatalogProviderEntry(
        provider_id="openrouter",
        display_name="OpenRouter",
        base_url="https://openrouter.ai/api/v1",
        protocol="openai",
        models_endpoint="https://openrouter.ai/api/v1/models",
        key_env_hint="openrouter.ai/keys",
        static_models=(),
    ),
    "mimo_token_plan": CatalogProviderEntry(
        provider_id="mimo_token_plan",
        display_name="Xiaomi MiMo Token Plan",
        # Placeholder pattern only — the real base URL is region-specific and
        # must be copied from the user's Token Plan dashboard.
        base_url="https://<region>.api.mimo.xiaomi.com/v1",
        protocol="openai",
        models_endpoint="",  # derived at login: {base_url}/models
        key_env_hint="Token Plan dashboard de Xiaomi MiMo → API Keys (copiá también tu base URL regional)",
        static_models=("mimo-v2.5", "mimo-v2.5-pro"),
        needs_base_url=True,
    ),
}


def catalog_provider_ids() -> list[str]:
    return list(CATALOG_PROVIDERS.keys())


def get_catalog_entry(provider_id: str) -> CatalogProviderEntry | None:
    return CATALOG_PROVIDERS.get(provider_id)


def catalog_entries_not_detected(detected_ids: set[str]) -> list[CatalogProviderEntry]:
    """Return catalog entries whose provider_id is NOT in detected_ids.

    Deduplication rule: if a provider is already in 'Tus proveedores'
    (detected automatically), it does NOT appear in the Catálogo.
    """
    return [
        entry for pid, entry in CATALOG_PROVIDERS.items() if pid not in detected_ids
    ]
