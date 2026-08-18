"""Nexum Provider Registry — declarative provider definitions (ADR-044 Capa 0).

Single source of truth for the providers Nexum knows about. Each entry tells the
detection layer where to find credentials, how to activate them, and how to discover
models. Adding a provider = adding an entry here; the detectors/catalog/commands
need no code changes.

Security: this module defines NO secret values — only paths, env var names, and
provider ids. It never reads files or network.

Stdlib only. No Nexum runtime imports.
"""

from __future__ import annotations

from dataclasses import dataclass, field


# ─── Auth modes ───────────────────────────────────────────────────────────────


class AuthMode:
    """How a provider authenticates (mirrors ADR-044 §3.1)."""

    CLI_OAUTH = "cli_oauth"  # native CLI login + CLIProxyAPI bridge
    DIRECT_KEY = "direct_key"  # OpenCode family: auth.json own key, no proxy
    STATIC_API_KEY = "static_api_key"  # "Populares": pasted key
    LOCAL_NO_AUTH = "local_no_auth"  # Ollama Local
    OPENAI_COMPATIBLE = "openai_compatible"  # custom OpenAI-compatible endpoint
    BRIDGE_PROXY = "bridge_proxy"  # routed via CLIProxyAPI bridge


# ─── Detection statuses ───────────────────────────────────────────────────────


class DetectionStatus:
    """Per-provider machine state (ADR-044 §3.2). Never mute — every non-USABLE
    state carries a detail + a next_action."""

    NOT_INSTALLED = "not_installed"
    NATIVE_LOGIN_DETECTED = "native_login_detected"
    BRIDGE_NOT_INSTALLED = "bridge_not_installed"
    BRIDGE_NOT_RUNNING = "bridge_not_running"
    BRIDGE_MANAGEMENT_LOCKED = "bridge_management_locked"
    BRIDGE_NOT_ACTIVE = "bridge_not_active"
    ACTIVATING = "activating"
    USABLE = "usable"
    EXPIRED = "expired"
    ERROR = "error"
    REQUIRES_API_KEY = "requires_api_key"
    REQUIRES_ADAPTER = "requires_adapter"
    PROBE_PENDING = "probe_pending"
    PROBE_FAILED = "probe_failed"
    NOT_CONFIGURED = "not_configured"


# Statuses considered "USABLE now" for /modelo selection.
USABLE_STATUSES = frozenset({DetectionStatus.USABLE})

# Statuses that indicate the provider is detectable but not yet usable.
DETECTED_NOT_USABLE_STATUSES = frozenset(
    {
        DetectionStatus.NATIVE_LOGIN_DETECTED,
        DetectionStatus.BRIDGE_NOT_INSTALLED,
        DetectionStatus.BRIDGE_NOT_RUNNING,
        DetectionStatus.BRIDGE_MANAGEMENT_LOCKED,
        DetectionStatus.BRIDGE_NOT_ACTIVE,
        DetectionStatus.ACTIVATING,
        DetectionStatus.EXPIRED,
        DetectionStatus.PROBE_PENDING,
        DetectionStatus.PROBE_FAILED,
    }
)


# ─── Model + provider definitions ─────────────────────────────────────────────


@dataclass(frozen=True)
class ModelInfo:
    """A known model for a provider (used when no live list endpoint exists)."""

    model_id: str
    display_name: str
    context_window: int | None = None


class StoreKind:
    """Formatos de almacén de credenciales que sabemos leer."""

    JSON_MAP = "json_map"  # {"<entrada>": {"key"|"access": "<secreto>"}}
    JSON_ACCOUNTS = "json_accounts"  # {"accounts": {id: {serviceID, credential}}}
    JSON_NESTED = "json_nested"  # {"<entrada>": {"<campo>": "<secreto>"}} anidado
    ENV = "env"  # variable de entorno exportada
    ENV_FILE = "env_file"  # archivo estilo KEY=valor
    YAML_LIST = "yaml_list"  # bloque `clave:` seguido de ítems `- valor`


@dataclass(frozen=True)
class CredentialStore:
    """Un lugar donde puede vivir la credencial de un provider.

    El ORDEN dentro de `credential_stores` es la precedencia, y espeja el orden
    que usa la CLI dueña de la credencial. Declarar los almacenes acá —en vez de
    hardcodear rutas en la lógica de resolución— es lo que permitió descubrir
    que a `opencode` se le estaba mirando el archivo equivocado.
    """

    kind: str
    path: str | None = None  # admite ~ y $XDG_DATA_HOME
    entries: tuple[str, ...] = ()  # entradas candidatas dentro del almacén
    fields: tuple[str, ...] = ("key", "access", "token")  # campos con el secreto
    env_vars: tuple[str, ...] = ()  # para ENV / ENV_FILE
    yaml_key: str | None = None  # para YAML_LIST
    legacy: bool = False  # almacén obsoleto: sólo fallback, se marca en el catálogo
    note: str = ""


class AccessKind:
    """Cómo se obtiene acceso a un subconjunto de modelos de un provider."""

    NO_CREDENTIAL = "no_credential"  # tier libre: responde sin Authorization
    API_KEY = "api_key"
    NATIVE_OAUTH = "native_oauth"
    ACCOUNT_LOGIN = "account_login"


@dataclass(frozen=True)
class AccessPath:
    """Un camino de acceso y QUÉ habilita.

    Un provider puede tener varios a la vez: OpenCode Zen sirve 7 modelos sin
    credencial y 52 con api-key. El modelo viejo asumía un provider = un
    estado, y eso es exactamente lo que no podía representar.
    """

    kind: str
    enables: str  # "pricing_free" | "pricing_paid" | "all"
    verify: str = "completion"  # cómo se prueba este camino puntual


@dataclass(frozen=True)
class ModelCatalogSource:
    """De dónde sale el pricing por modelo.

    Lista con precedencia, igual que `credential_stores` y `access_paths`: hoy
    tiene un elemento, y sumar un respaldo (p. ej. models.dev) es una línea de
    config y no un rediseño.
    """

    kind: str  # "local_cache"
    path: str | None = None
    namespace: str | None = None  # entrada del provider dentro del catálogo
    note: str = ""


@dataclass(frozen=True)
class ProviderDefinition:
    """Declarative definition of a provider.

    Adding a provider = adding an entry to PROVIDER_REGISTRY. The detectors,
    catalog builder, and commands adapt automatically based on `auth_mode`.
    """

    provider_id: str
    family: str  # /modelo section header, e.g. "OpenCode Go"
    display_name: str  # /proveedor row name
    auth_mode: str  # AuthMode.*
    # CLI_OAUTH / DIRECT_KEY: where the native credential lives.
    native_credential_paths: tuple[str, ...] = field(default_factory=tuple)
    # CLI_OAUTH: the provider id CLIProxyAPI uses in GET /auth-files and the
    # suffix of GET /{provider}-auth-url.
    cliproxy_provider_id: str | None = None
    # CLI_OAUTH: the `owned_by` tag the bridge stamps on each model in its
    # OpenAI-compatible GET /v1/models. Partitions the shared model list into
    # per-provider sections. NOT the same as `cliproxy_provider_id`: the bridge
    # reports Codex models as owned_by="openai", not "codex".
    # Presence of this field is what makes a provider eligible for the direct
    # resolver (see reconcile._resolver_source) — no provider id is hardcoded.
    bridge_model_owner: str | None = None
    # DIRECT_KEY: keys inside auth.json that correspond to this provider
    # (an OpenCode-family auth.json can have several entries).
    auth_json_keys: tuple[str, ...] = field(default_factory=tuple)
    # Known models if the provider has no live list endpoint.
    static_models: tuple[ModelInfo, ...] = field(default_factory=tuple)
    # Env var names whose presence indicates a static API key.
    env_vars: tuple[str, ...] = field(default_factory=tuple)
    # Optional base URL hint (for probes / display).
    base_url_hint: str | None = None
    # How models are discovered.
    model_discovery_strategy: str = "static"
    # Human-readable category for /proveedor grouping.
    category: str = "cloud"
    # Whether this is a recommended/popular provider.
    recommended: bool = False
    # Free-form description.
    description: str | None = None
    # Almacenes de credenciales EN ORDEN DE PRECEDENCIA (el primero que
    # resuelve, gana). Vacío = el provider no tiene credencial propia que leer.
    credential_stores: tuple[CredentialStore, ...] = field(default_factory=tuple)
    # Endpoint que valida la credencial: devuelve 401 sin ella y 200 con ella.
    # Cuando existe, la verificación NO cuesta tokens.
    verify_endpoint: str | None = None
    # Marcador de los modelos de acceso libre (tier gratuito). Verificado el
    # 2026-07-25 contra opencode.ai: los modelos con este sufijo responden 200
    # a `chat/completions` SIN credencial, y son los que la CLI `opencode`
    # expone gratis con sólo instalarla.
    free_model_marker: str | None = None
    # Caminos de acceso, en orden de precedencia. Vacío = comportamiento previo.
    access_paths: tuple[AccessPath, ...] = field(default_factory=tuple)
    # Fuentes de pricing por modelo, en orden de precedencia.
    model_catalog_sources: tuple[ModelCatalogSource, ...] = field(default_factory=tuple)
    # Alias históricos: ids que se colapsaron en esta entrada y que no deben
    # romper referencias existentes.
    aliases: tuple[str, ...] = field(default_factory=tuple)


# ─── The registry ─────────────────────────────────────────────────────────────
#
# 17 providers per ADR-044 + spec FASE 2. Auth modes drive detector dispatch:
#   cli_oauth       → CLIOAuthDetector (native login + CLIProxyAPI bridge)
#   direct_key      → OpenCodeFamilyDetector (auth.json extraction + probe)
#   static_api_key  → static (env var presence)
#   local_no_auth   → OllamaDetector (conserved, probe localhost)
#   openai_compatible → custom (base_url + key)


# Almacenes de la familia OpenCode, en el orden que usa la propia CLI.
#
# Verificado en el binario de opencode 1.17.13:
#     function o5(){ ... return join($, ".local","share","opencode","auth.json") }
# `auth.json` es el almacén que la CLI lee. `account.json` (formato "version": 2)
# NO aparece como ruta de credenciales: es un almacén viejo que quedó en disco y
# que en esta máquina conserva una key ya rotada. Por eso va como `legacy`.
#
# El entorno va último a propósito: la key exportada en ~/.bashrc resultó
# INVÁLIDA mientras las de auth.json seguían siendo válidas.
def _opencode_stores(entries: tuple[str, ...], env_vars: tuple[str, ...] = ()):
    stores = [
        CredentialStore(
            kind=StoreKind.JSON_MAP,
            path="$XDG_DATA_HOME/opencode/auth.json",
            entries=entries,
            note="almacén autoritativo de la CLI opencode",
        ),
        CredentialStore(
            kind=StoreKind.JSON_ACCOUNTS,
            path="$XDG_DATA_HOME/opencode/account.json",
            entries=entries,
            legacy=True,
            note="formato v2 obsoleto; puede contener keys rotadas",
        ),
    ]
    if env_vars:
        stores.append(
            CredentialStore(
                kind=StoreKind.ENV,
                env_vars=env_vars,
                legacy=True,
                note="último recurso: el entorno puede tener una key vieja",
            )
        )
    return tuple(stores)


PROVIDER_REGISTRY: dict[str, ProviderDefinition] = {
    # ── Local (conserved) ──
    "ollama_local": ProviderDefinition(
        provider_id="ollama_local",
        family="Ollama Local",
        display_name="Ollama Local",
        auth_mode=AuthMode.LOCAL_NO_AUTH,
        base_url_hint="http://127.0.0.1:11434/v1",
        model_discovery_strategy="openai_compatible_models_endpoint",
        category="local",
        recommended=True,
        description="Local Ollama, OpenAI-compatible /v1/models.",
    ),
    # ── CLI OAuth families (need CLIProxyAPI bridge to become usable) ──
    "claude_code": ProviderDefinition(
        provider_id="claude_code",
        family="Claude",
        display_name="Claude Code",
        auth_mode=AuthMode.CLI_OAUTH,
        native_credential_paths=("~/.claude/.credentials.json",),
        cliproxy_provider_id="anthropic",
        bridge_model_owner="anthropic",
        static_models=(
            ModelInfo("claude-opus-4-8", "Claude Opus 4.8"),
            ModelInfo("claude-fable-5", "Claude Fable 5"),
            ModelInfo("claude-opus-4-20250514", "Claude Opus 4"),
            ModelInfo("claude-3-5-haiku-20241022", "Claude 3.5 Haiku"),
            ModelInfo("claude-opus-4-6", "Claude Opus 4.6"),
            ModelInfo("claude-opus-4-7", "Claude Opus 4.7"),
            ModelInfo("claude-sonnet-5", "Claude Sonnet 5"),
            ModelInfo("claude-sonnet-4-5-20250929", "Claude Sonnet 4.5"),
            ModelInfo("claude-sonnet-4-6", "Claude Sonnet 4.6"),
            ModelInfo("claude-3-7-sonnet-20250219", "Claude 3.7 Sonnet"),
            ModelInfo("claude-opus-4-5-20251101", "Claude Opus 4.5"),
            ModelInfo("claude-opus-4-1-20250805", "Claude Opus 4.1"),
            ModelInfo("claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
            ModelInfo("claude-opus-4-6-thinking", "Claude Opus 4.6 Thinking"),
        ),
        model_discovery_strategy="cliproxy_models_or_static",
        category="cli_login",
    ),
    "codex_cli": ProviderDefinition(
        provider_id="codex_cli",
        family="Codex / OpenAI",
        display_name="Codex / OpenAI",
        auth_mode=AuthMode.CLI_OAUTH,
        native_credential_paths=("~/.codex/auth.json", "~/.codex/config.toml"),
        cliproxy_provider_id="codex",
        bridge_model_owner="openai",
        # Fallback si el bridge no responde /v1/models al momento del regen:
        # usable sin modelos no sirve en /modelo (bug post-login 2026-07-06).
        # Ids verificados contra el /v1/models real del puente.
        static_models=(
            ModelInfo("gpt-oss-120b-medium", "GPT OSS 120B Medium"),
            ModelInfo("gpt-5.5", "GPT-5.5"),
            ModelInfo("gpt-image-1.5", "GPT Image 1.5"),
            ModelInfo("gpt-5.4-mini", "GPT-5.4 Mini"),
            ModelInfo("codex-auto-review", "Codex Auto Review"),
            ModelInfo("gpt-5.6-terra", "GPT-5.6 Terra"),
            ModelInfo("gpt-5.4", "GPT-5.4"),
            ModelInfo("gpt-image-2", "GPT Image 2"),
            ModelInfo("gpt-5.6-luna", "GPT-5.6 Luna"),
            ModelInfo("gpt-5.3-codex-spark", "GPT-5.3 Codex Spark"),
            ModelInfo("gpt-5.6-sol", "GPT-5.6 Sol"),
        ),
        model_discovery_strategy="cliproxy_models_or_static",
        category="cli_login",
    ),
    "gemini_cli": ProviderDefinition(
        provider_id="gemini_cli",
        family="Gemini",
        display_name="Gemini CLI",
        auth_mode=AuthMode.CLI_OAUTH,
        native_credential_paths=(
            "~/.gemini/oauth_creds.json",
            "~/.gemini/settings.json",
        ),
        # CLIProxyAPI 7.2.50 (instalado en esta máquina) no expone
        # `gemini-auth-url`: el login de Google/Gemini se hace vía el flujo
        # Antigravity (`antigravity-auth-url`), que puentea modelos Gemini.
        # Verificado contra `cli-proxy-api --help` y strings del binario.
        cliproxy_provider_id="antigravity",
        bridge_model_owner="antigravity",
        static_models=(
            ModelInfo("gpt-oss-120b-medium", "GPT OSS 120B Medium"),
            ModelInfo("gemini-pro-agent", "Gemini Pro Agent"),
            ModelInfo("gemini-3.5-flash-extra-low", "Gemini 3.5 Flash Extra Low"),
            ModelInfo("gemini-3.1-flash-image", "Gemini 3.1 Flash Image"),
            ModelInfo("gemini-3.1-pro-low", "Gemini 3.1 Pro Low"),
            ModelInfo("claude-sonnet-4-6", "Claude Sonnet 4.6"),
            ModelInfo("gemini-3-flash", "Gemini 3 Flash"),
            ModelInfo("gemini-3-flash-agent", "Gemini 3 Flash Agent"),
            ModelInfo("gemini-3.5-flash-low", "Gemini 3.5 Flash Low"),
            ModelInfo("gemini-3.1-flash-lite", "Gemini 3.1 Flash Lite"),
            ModelInfo("claude-opus-4-6-thinking", "Claude Opus 4.6 Thinking"),
        ),
        model_discovery_strategy="cliproxy_models_or_static",
        category="cli_login",
    ),
    # ── OpenCode family (DIRECT_KEY from auth.json, no proxy) ──
    # ── OpenCode Free (`opencode_zen`) y OpenCode Go permanecen separados ──
    #
    # CAUSA RAÍZ del enredo de OpenCode: el registry tenía las base URLs
    # CRUZADAS — `opencode` apuntaba a zen/v1 (la de Zen) y `opencode_zen` a
    # zen/go/v1 (la de Go). Un solo bug de configuración con tres síntomas:
    # los modelos gratis colgaban de la fila equivocada y Go heredaba un
    # diagnóstico ajeno. Se conserva el ID histórico `opencode_zen` por
    # compatibilidad, con nombre visible OpenCode Free; `opencode_go` conserva
    # su fila, modelos y cuota independientes.
    "opencode_zen": ProviderDefinition(
        provider_id="opencode_zen",
        family="OpenCode Free",
        display_name="OpenCode Free",
        auth_mode=AuthMode.DIRECT_KEY,
        native_credential_paths=("~/.local/share/opencode/auth.json",),
        auth_json_keys=(
            "opencode-zen",
            "opencode_zen",
            "zen",
            "opencode",
            "opencode-go",
        ),
        aliases=("opencode",),
        credential_stores=_opencode_stores(
            ("opencode-zen", "opencode_zen", "zen", "opencode", "opencode-go"),
            ("OPENCODE_ZEN_API_KEY",),
        ),
        base_url_hint="https://opencode.ai/zen/v1",
        verify_endpoint="https://opencode.ai/zen/v1/chat/completions",
        model_discovery_strategy="openai_compatible_models_endpoint",
        static_models=(
            ModelInfo("big-pickle", "Big Pickle"),
            ModelInfo("deepseek-v4-flash-free", "DeepSeek V4 Flash Free"),
            ModelInfo("ling-3.0-flash-free", "Ling 3.0 Flash Free"),
            ModelInfo("mimo-v2.5-free", "MiMo V2.5 Free"),
            ModelInfo("nemotron-3-ultra-free", "Nemotron 3 Ultra Free"),
            ModelInfo("north-mini-code-free", "North Mini Code Free"),
        ),
        # Dos caminos conviven en la MISMA fila: sin credencial se sirven los
        # gratuitos, con api-key los de pago.
        access_paths=(
            AccessPath(kind=AccessKind.NO_CREDENTIAL, enables="pricing_free"),
            AccessPath(kind=AccessKind.API_KEY, enables="pricing_paid"),
        ),
        model_catalog_sources=(
            ModelCatalogSource(
                kind="local_cache",
                path="$XDG_CACHE_HOME/opencode/models.json",
                namespace="opencode",
                note="catálogo de la CLI (models.dev): trae cost por modelo",
            ),
        ),
        category="aggregator",
        recommended=True,
        description="Modelos gratuitos y de pago vía OpenCode Zen.",
    ),
    "opencode_go": ProviderDefinition(
        provider_id="opencode_go",
        family="OpenCode Go",
        display_name="OpenCode Go",
        auth_mode=AuthMode.DIRECT_KEY,
        native_credential_paths=("~/.local/share/opencode/auth.json",),
        auth_json_keys=("opencode-go", "opencode_go", "go"),
        model_discovery_strategy="openai_compatible_models_endpoint",
        base_url_hint="https://opencode.ai/zen/go/v1",
        credential_stores=_opencode_stores(
            ("opencode-go", "opencode_go", "go"), ("OPENCODE_GO_API_KEY",)
        ),
        verify_endpoint="https://opencode.ai/zen/go/v1/chat/completions",
        static_models=(
            ModelInfo("deepseek-v4-flash", "DeepSeek V4 Flash"),
            ModelInfo("deepseek-v4-pro", "DeepSeek V4 Pro"),
            ModelInfo("glm-5.1", "GLM 5.1"),
            ModelInfo("glm-5.2", "GLM 5.2"),
            ModelInfo("kimi-k2.6", "Kimi K2.6"),
            ModelInfo("kimi-k2.7-code", "Kimi K2.7 Code"),
            ModelInfo("mimo-v2.5", "MiMo V2.5"),
            ModelInfo("mimo-v2.5-pro", "MiMo V2.5 Pro"),
            ModelInfo("minimax-m2.7", "MiniMax M2.7"),
            ModelInfo("minimax-m3", "MiniMax M3"),
            ModelInfo("qwen3.6-plus", "Qwen 3.6 Plus"),
            ModelInfo("qwen3.7-max", "Qwen 3.7 Max"),
            ModelInfo("qwen3.7-plus", "Qwen 3.7 Plus"),
        ),
        # Suscripción SEPARADA de la cuenta compartida: workspace propio y
        # cuota mensual propia. Su 429 "monthly usage limit" no es el
        # "insufficient balance" de Zen.
        access_paths=(AccessPath(kind=AccessKind.API_KEY, enables="all"),),
        category="aggregator",
        description="OpenCode Go subscription (~$10/mo, open-source models).",
    ),
    # ── MiMo Code (suscripción Xiaomi — auth.json propio desde jun 2026) ──
    "mimo_code": ProviderDefinition(
        provider_id="mimo_code",
        family="MiMo",
        display_name="MiMo",
        # Sprint C: al loguearse en MiMo Code aparece
        # ~/.local/share/mimocode/auth.json con entrada "xiaomi" (type api,
        # key + metadata.base_url regional). Mismo mecanismo que la familia
        # OpenCode; el hallazgo de Sprint 1 ("solo SQLite/keyring") quedó
        # obsoleto — mimo_detector.py queda como fallback sin login.
        auth_mode=AuthMode.DIRECT_KEY,
        native_credential_paths=("~/.local/share/mimocode/auth.json",),
        auth_json_keys=("xiaomi",),
        credential_stores=(
            CredentialStore(
                kind=StoreKind.JSON_NESTED,
                path="$XDG_DATA_HOME/mimocode/auth.json",
                entries=("xiaomi",),
                note="auth.json propio de MiMo Code; base_url regional en metadata",
            ),
        ),
        # /models devuelve 401 sin key y 200 con key: verificación sin costo.
        verify_endpoint="{base_url}/models",
        # La base URL real por usuario viene en entry.metadata.base_url
        # (p.ej. token-plan-sgp.xiaomimimo.com); este hint es el fallback.
        base_url_hint="https://api.xiaomimimo.com/v1",
        model_discovery_strategy="openai_compatible_models_endpoint",
        static_models=(
            ModelInfo("mimo-v2.5", "MiMo V2.5"),
            ModelInfo("mimo-v2.5-asr", "MiMo V2.5 ASR"),
            ModelInfo("mimo-v2.5-pro", "MiMo V2.5 Pro"),
            ModelInfo("mimo-v2.5-tts", "MiMo V2.5 TTS"),
            ModelInfo("mimo-v2.5-tts-voiceclone", "MiMo V2.5 TTS Voice Clone"),
            ModelInfo("mimo-v2.5-tts-voicedesign", "MiMo V2.5 TTS Voice Design"),
        ),
        category="cli_login",
        recommended=True,
        description="Suscripción MiMo Code (Xiaomi) — key extraída de su auth.json.",
    ),
    # ── Static API key ("Populares" fallback) ──
    "anthropic_api_key": ProviderDefinition(
        provider_id="anthropic_api_key",
        family="Claude (API key manual)",
        display_name="Anthropic / Claude",
        auth_mode=AuthMode.STATIC_API_KEY,
        env_vars=("ANTHROPIC_API_KEY", "CLAUDE_API_KEY"),
        base_url_hint="https://api.anthropic.com",
        model_discovery_strategy="anthropic_models_or_static_until_probe",
        category="cloud",
    ),
    "google_api_key": ProviderDefinition(
        provider_id="google_api_key",
        family="Gemini (API key manual)",
        display_name="Google / Gemini",
        auth_mode=AuthMode.STATIC_API_KEY,
        env_vars=("GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"),
        model_discovery_strategy="google_models_or_static_until_probe",
        category="cloud",
    ),
    "openrouter": ProviderDefinition(
        provider_id="openrouter",
        family="OpenRouter",
        display_name="OpenRouter",
        auth_mode=AuthMode.STATIC_API_KEY,
        env_vars=("OPENROUTER_API_KEY",),
        base_url_hint="https://openrouter.ai/api/v1",
        model_discovery_strategy="openai_compatible_models_endpoint",
        category="cloud_gateway",
    ),
    "deepseek": ProviderDefinition(
        provider_id="deepseek",
        family="DeepSeek",
        display_name="DeepSeek",
        auth_mode=AuthMode.STATIC_API_KEY,
        env_vars=("DEEPSEEK_API_KEY",),
        base_url_hint="https://api.deepseek.com/v1",
        model_discovery_strategy="openai_compatible_models_endpoint",
        category="cloud",
    ),
    "glm": ProviderDefinition(
        provider_id="glm",
        family="Zhipu / GLM",
        display_name="Zhipu AI (GLM)",
        auth_mode=AuthMode.STATIC_API_KEY,
        env_vars=("ZAI_API_KEY", "GLM_API_KEY"),
        base_url_hint="https://open.bigmodel.cn/api/paas/v4",
        model_discovery_strategy="openai_compatible_models_endpoint",
        category="cloud",
    ),
    "glm_coding_plan": ProviderDefinition(
        provider_id="glm_coding_plan",
        family="Z.ai / GLM Coding Plan",
        display_name="Z.ai / GLM Coding Plan",
        auth_mode=AuthMode.STATIC_API_KEY,
        env_vars=("ZAI_CODING_API_KEY",),
        # Sprint 2: Z.ai Coding Plan uses different base URLs than general API.
        # Anthropic protocol: https://api.z.ai/api/anthropic
        # OpenAI protocol (Coding Plan): https://api.z.ai/api/coding/paas/v4
        # WARNING: Do NOT use /paas/v4 (general API) — Coding Plan models require /coding/paas/v4.
        base_url_hint="https://api.z.ai/api/coding/paas/v4",
        static_models=(
            ModelInfo("glm-5.2", "GLM-5.2", 128000),
            ModelInfo("glm-5-turbo", "GLM-5-Turbo", 200000),
        ),
        model_discovery_strategy="openai_compatible_models_endpoint",
        category="cloud",
        recommended=True,
        description=(
            "GLM-5.2 y modelos de Coding Plan vía Z.ai. "
            "API key en z.ai/manage-apikey/apikey-list. "
            "Usa base URL /coding/paas/v4 (no /paas/v4)."
        ),
    ),
    "qwen": ProviderDefinition(
        provider_id="qwen",
        family="Qwen",
        display_name="Qwen",
        auth_mode=AuthMode.STATIC_API_KEY,
        env_vars=("DASHSCOPE_API_KEY",),
        base_url_hint="https://dashscope.aliyuncs.com/compatible-mode/v1",
        model_discovery_strategy="openai_compatible_models_endpoint",
        category="cloud",
    ),
    # ── Custom OpenAI-compatible ──
    "custom_openai_compatible": ProviderDefinition(
        provider_id="custom_openai_compatible",
        family="Proveedor personalizado",
        display_name="Proveedor personalizado",
        auth_mode=AuthMode.OPENAI_COMPATIBLE,
        env_vars=("OPENAI_API_KEY",),
        base_url_hint=None,  # user-supplied
        model_discovery_strategy="openai_compatible_models_endpoint",
        category="custom",
        description="Custom OpenAI-compatible endpoint (base URL + API key).",
    ),
    # ── Future / not-yet-supported ──
    "github_copilot": ProviderDefinition(
        provider_id="github_copilot",
        family="GitHub Copilot",
        display_name="GitHub Copilot",
        auth_mode=AuthMode.CLI_OAUTH,
        cliproxy_provider_id=None,  # not yet wired
        model_discovery_strategy="future_adapter",
        category="cli_or_oauth",
        description="Requires a future adapter.",
    ),
    "vercel_ai_gateway": ProviderDefinition(
        provider_id="vercel_ai_gateway",
        family="Vercel AI Gateway",
        display_name="Vercel AI Gateway",
        auth_mode=AuthMode.STATIC_API_KEY,
        env_vars=("AI_GATEWAY_API_KEY", "VERCEL_AI_GATEWAY_API_KEY"),
        base_url_hint="https://ai-gateway.vercel.sh/v1",
        model_discovery_strategy="openai_compatible_models_endpoint",
        category="cloud_gateway",
    ),
}


def providers_by_auth_mode(auth_mode: str) -> list[ProviderDefinition]:
    """Return all registry entries matching an auth mode."""
    return [d for d in PROVIDER_REGISTRY.values() if d.auth_mode == auth_mode]


def get(provider_id: str) -> ProviderDefinition | None:
    return PROVIDER_REGISTRY.get(provider_id)


def provider_ids() -> list[str]:
    return list(PROVIDER_REGISTRY.keys())
