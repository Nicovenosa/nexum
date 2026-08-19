//! /provedor — Provider Catalog panel (ADR-044 cierre).
//!
//! Renderiza el catálogo que resuelve `nexum_acp::provider::catalog_path` —el
//! vivo de `reconcile` cuando existe, no la copia congelada del slot. This panel does NOT reuse LoginPanel and therefore
//! never shows inherited/fake data (no Opus/Sonnet/Haiku, no "(openai)" subtitle, no
//! reserved `qwen3:0.6b` as a user-facing model). The catalog JSON is the single source
//! of truth; if it is missing the panel shows an explicit error (no fallback, no
//! hardcoded data, no auto-shell-out).
//!
//! Two sections (ADR-044 cierre):
//!   - "Tus proveedores": everything detected on this machine, with live state.
//!   - "Catálogo": pre-configured providers (from the catalog JSON `catalog`
//!     array). Enter on a row opens a masked API-key input; the key is validated
//!     live by `src/nexum_providers/provider_login.py` (probe → KeyStore →
//!     catalog upsert) and on success the provider jumps to "Tus proveedores"
//!     and its models appear in /modelo without restarting.
//!
//! Security: the API key lives only in panel memory and travels to Python via
//! stdin. It is never logged, never rendered (masked ••••), never in argv.
//!
//! Reserved model policy is enforced upstream by the catalog (reserved models are
//! excluded from `model_policy.selectable_models`/`user_facing_models`); this panel
//! only displays the counts and never names a reserved model as selectable.

use std::any::Any;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Instant;

use ratatui::{layout::Rect, Frame};
use serde::Deserialize;
use tui_textarea::Input;

use super::{
    panel_component::PanelComponent,
    panel_manager::{EventResult, PanelContext, PanelKind},
    App,
};

// Los nombres y la resolución viven en nexum-acp: FUENTE ÚNICA.
pub(crate) use nexum_acp::provider::catalog_path::{CatalogResolution, CatalogSource};

/// Product resources come only from a validated InstalledLayoutV1.
pub(crate) fn provider_resource_root() -> Option<std::path::PathBuf> {
    crate::layout::InstalledLayoutV1::current().map(|layout| layout.version_root())
}

/// Catálogo base empaquetado en los recursos instalados.
fn catalog_installed_base() -> Option<std::path::PathBuf> {
    crate::layout::InstalledLayoutV1::current().map(|layout| layout.base_catalog())
}

// ─── Serde model del catálogo de providers ────────────────────────────────────
//
// All fields are optional/defaulted so a partial or future-shaped catalog still
// renders rather than crashing the panel. We only deserialize what we display.

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CatalogModelPolicy {
    #[serde(default)]
    pub user_facing_models: Vec<String>,
    #[serde(default)]
    pub user_facing_count: Option<i64>,
    #[serde(default)]
    pub reserved_internal_count: Option<i64>,
    #[serde(default)]
    pub all_models_reserved: Option<bool>,
    #[serde(default)]
    pub selectable_models: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CatalogProviderEntry {
    pub id: String,
    /// Tolerante al parsear, exigido al validar: `validate_at` lo reporta
    /// como RequiredFieldMissing tipado en vez de morir en el parse, y los
    /// fixtures mínimos de test no necesitan repetirlo. `name()` cae al id.
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub connection_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub status_detail: String,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub credential_detected: bool,
    #[serde(default)]
    pub credential_fingerprint: Option<String>,
    #[serde(default)]
    pub base_url_detected: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub models_detected: Vec<String>,
    #[serde(default)]
    pub model_policy: CatalogModelPolicy,
    // ── ADR-044 v2 fields (all optional, backward-compatible). ──
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub auth_mode: Option<String>,
    /// Cómo se conecta este provider, derivado por el catálogo desde su
    /// `auth_mode`: "bridge_oauth" | "credential_store" | "api_key_input".
    /// El panel enruta la acción por este campo en vez de por una lista de ids
    /// escrita a mano, que dejaba a 4 providers sin ninguna acción posible.
    #[serde(default)]
    pub connect_kind: Option<String>,
    /// Comando con el que el usuario vuelve a loguearse en la CLI dueña de la
    /// credencial (p. ej. `opencode auth login`). Se muestra, no se ejecuta.
    #[serde(default)]
    pub relogin_command: Option<String>,
    /// Ruta (sanitizada) del almacén del que salió la credencial. En esta
    /// máquina conviven cuatro valores distintos de la key de OpenCode: saber
    /// cuál se usó es la diferencia entre diagnosticar y adivinar.
    #[serde(default)]
    pub credential_store: Option<String>,
    /// Veredicto legible de la verificación ("credencial válida, cuenta sin
    /// saldo", "credencial rechazada", …).
    #[serde(default)]
    pub credential_detail: Option<String>,
    /// Veredicto de la verificación: "verified" | "verified_no_credit" |
    /// "present_unverified" | "invalid" | "free_access". Decide la sección.
    #[serde(default)]
    pub credential_state: Option<String>,
    #[serde(default)]
    pub native_login_detected: Option<bool>,
    #[serde(default)]
    pub bridge_status: Option<String>,
    #[serde(default)]
    pub bridge_detail: Option<String>,
    #[serde(default)]
    pub usable_now: Option<bool>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub models_status: Option<String>,
    #[serde(default)]
    pub next_action: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub last_refresh: Option<String>,
}

impl CatalogProviderEntry {
    /// Display name falls back to id if display_name is empty (robustness).
    pub(crate) fn name(&self) -> &str {
        if self.display_name.is_empty() {
            &self.id
        } else {
            &self.display_name
        }
    }

    /// Family (v2) falls back to display_name (v1) for backward compat.
    pub(crate) fn family_or_name(&self) -> &str {
        self.family.as_deref().unwrap_or_else(|| self.name())
    }

    /// True if this provider is usable now (v2 `usable_now` field, with v1
    /// `status == "usable_now"` as fallback).
    pub(crate) fn is_usable(&self) -> bool {
        self.usable_now
            .unwrap_or_else(|| self.status == "usable_now")
    }

    pub(crate) fn stable_id(&self) -> &str {
        self.provider_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .unwrap_or(&self.id)
    }

    pub(crate) fn user_facing_models(&self) -> &[String] {
        if !self.model_policy.user_facing_models.is_empty() {
            &self.model_policy.user_facing_models
        } else if !self.model_policy.selectable_models.is_empty() {
            &self.model_policy.selectable_models
        } else {
            &self.models
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CliProxyApiInfo {
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub port: i64,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

/// A pre-configured provider from the catalog JSON `catalog` array
/// (one-step login: paste an API key, probe, done).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub(crate) struct CatalogLoginEntry {
    pub provider_id: String,
    pub display_name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub protocol: String,
    #[serde(default)]
    pub key_env_hint: String,
    #[serde(default)]
    pub needs_base_url: bool,
    #[serde(default)]
    pub static_models: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CatalogDoc {
    #[serde(default)]
    pub schema_version: Option<u32>,
    #[serde(default)]
    pub catalog_version: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub recommended_provider_id: Option<String>,
    #[serde(default)]
    pub active_provider_id: Option<String>,
    #[serde(default)]
    pub providers: Vec<CatalogProviderEntry>,
    #[serde(default)]
    pub catalog: Vec<CatalogLoginEntry>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub cli_proxy_api: Option<CliProxyApiInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogContract {
    catalog_schema_version: u32,
    #[serde(default)]
    required_providers_with_models: Vec<String>,
    #[serde(default)]
    manual_provider_ids: Vec<String>,
}

fn catalog_contract() -> Result<CatalogContract, CatalogLoadError> {
    serde_json::from_str(include_str!("../../../config/catalog-contract.json")).map_err(|error| {
        CatalogLoadError::ContractInvalid {
            detail: error.to_string(),
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CatalogLoadError {
    Read {
        path: PathBuf,
        detail: String,
    },
    JsonParse {
        path: PathBuf,
        line: usize,
        column: usize,
        detail: String,
    },
    ContractInvalid {
        detail: String,
    },
    SchemaVersionMismatch {
        path: PathBuf,
        expected: u32,
        observed: Option<u32>,
    },
    RequiredFieldMissing {
        path: PathBuf,
        field: String,
    },
    ProviderEntryInvalid {
        path: PathBuf,
        provider_id: String,
        field: String,
        detail: String,
    },
    ModelEntryInvalid {
        path: PathBuf,
        provider_id: String,
        field: String,
        detail: String,
    },
    DuplicateProviderId {
        path: PathBuf,
        provider_id: String,
    },
    DuplicateModelId {
        path: PathBuf,
        provider_id: String,
        model_id: String,
    },
}

impl CatalogLoadError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Read { .. } => "PATH_RESOLUTION_ERROR",
            Self::JsonParse { .. } => "JSON_PARSE_ERROR",
            Self::ContractInvalid { .. } => "CATALOG_BINARY_VERSION_SKEW",
            Self::SchemaVersionMismatch { .. } => "SCHEMA_VERSION_MISMATCH",
            Self::RequiredFieldMissing { .. } => "REQUIRED_FIELD_MISSING",
            Self::ProviderEntryInvalid { .. } => "PROVIDER_ENTRY_INVALID",
            Self::ModelEntryInvalid { .. } => "MODEL_ENTRY_INVALID",
            Self::DuplicateProviderId { .. } => "DUPLICATE_PROVIDER_ID",
            Self::DuplicateModelId { .. } => "DUPLICATE_MODEL_ID",
        }
    }
}

impl fmt::Display for CatalogLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::Read { path, detail } => {
                write!(f, "path={} detail={detail}", path.display())
            }
            Self::JsonParse {
                path,
                line,
                column,
                detail,
            } => write!(
                f,
                "path={} line={line} column={column} detail={detail}",
                path.display()
            ),
            Self::ContractInvalid { detail } => write!(f, "contract detail={detail}"),
            Self::SchemaVersionMismatch {
                path,
                expected,
                observed,
            } => write!(
                f,
                "path={} field=schema_version expected={expected} observed={observed:?}",
                path.display()
            ),
            Self::RequiredFieldMissing { path, field } => {
                write!(f, "path={} field={field}", path.display())
            }
            Self::ProviderEntryInvalid {
                path,
                provider_id,
                field,
                detail,
            }
            | Self::ModelEntryInvalid {
                path,
                provider_id,
                field,
                detail,
            } => write!(
                f,
                "path={} provider_id={provider_id} field={field} detail={detail}",
                path.display()
            ),
            Self::DuplicateProviderId { path, provider_id } => {
                write!(f, "path={} provider_id={provider_id}", path.display())
            }
            Self::DuplicateModelId {
                path,
                provider_id,
                model_id,
            } => write!(
                f,
                "path={} provider_id={provider_id} model_id={model_id}",
                path.display()
            ),
        }
    }
}

impl CatalogDoc {
    fn validate_at(&self, path: &Path) -> Result<(), CatalogLoadError> {
        let contract = catalog_contract()?;
        if self.schema_version != Some(contract.catalog_schema_version) {
            return Err(CatalogLoadError::SchemaVersionMismatch {
                path: path.to_path_buf(),
                expected: contract.catalog_schema_version,
                observed: self.schema_version,
            });
        }
        if self.providers.is_empty() {
            return Err(CatalogLoadError::RequiredFieldMissing {
                path: path.to_path_buf(),
                field: "providers".to_string(),
            });
        }
        let mut ids = HashSet::new();
        for (index, provider) in self.providers.iter().enumerate() {
            let id = provider.stable_id();
            if id.is_empty() {
                return Err(CatalogLoadError::RequiredFieldMissing {
                    path: path.to_path_buf(),
                    field: format!("providers[{index}].provider_id"),
                });
            }
            if !ids.insert(id.to_string()) {
                return Err(CatalogLoadError::DuplicateProviderId {
                    path: path.to_path_buf(),
                    provider_id: id.to_string(),
                });
            }
            if provider.name().is_empty() {
                return Err(CatalogLoadError::RequiredFieldMissing {
                    path: path.to_path_buf(),
                    field: format!("providers[{index}].display_name"),
                });
            }
            let mut model_ids = HashSet::new();
            for (model_index, model_id) in provider.user_facing_models().iter().enumerate() {
                if model_id.trim().is_empty() {
                    return Err(CatalogLoadError::ModelEntryInvalid {
                        path: path.to_path_buf(),
                        provider_id: id.to_string(),
                        field: format!("providers[{index}].models[{model_index}]"),
                        detail: "model_id must be non-empty".to_string(),
                    });
                }
                if !model_ids.insert(model_id) {
                    return Err(CatalogLoadError::DuplicateModelId {
                        path: path.to_path_buf(),
                        provider_id: id.to_string(),
                        model_id: model_id.to_string(),
                    });
                }
            }
        }
        for required in &contract.required_providers_with_models {
            let provider =
                self.provider(required)
                    .ok_or_else(|| CatalogLoadError::RequiredFieldMissing {
                        path: path.to_path_buf(),
                        field: format!("providers[provider_id={required}]"),
                    })?;
            if provider.user_facing_models().is_empty() {
                let index = self
                    .providers
                    .iter()
                    .position(|candidate| candidate.stable_id() == required)
                    .unwrap_or(0);
                return Err(CatalogLoadError::ProviderEntryInvalid {
                    path: path.to_path_buf(),
                    provider_id: required.clone(),
                    field: format!("providers[{index}].models"),
                    detail: "required provider must expose selectable models".to_string(),
                });
            }
        }
        for manual_provider_id in &contract.manual_provider_ids {
            if self.provider(manual_provider_id).is_none() {
                return Err(CatalogLoadError::RequiredFieldMissing {
                    path: path.to_path_buf(),
                    field: format!("providers[provider_id={manual_provider_id}]"),
                });
            }
        }
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result<(), CatalogLoadError> {
        self.validate_at(Path::new("<memory>"))
    }

    pub(crate) fn provider(&self, provider_id: &str) -> Option<&CatalogProviderEntry> {
        let normalized = provider_id.replace('-', "_");
        self.providers
            .iter()
            .find(|provider| provider.stable_id().replace('-', "_") == normalized)
    }
}

pub(crate) struct CatalogSummary {
    pub(crate) schema_version: Option<u32>,
    pub(crate) provider_count: usize,
    pub(crate) base_provider_count: usize,
    pub(crate) manual_provider_count: usize,
    pub(crate) model_count: usize,
}

pub(crate) fn catalog_summary(doc: &CatalogDoc) -> Result<CatalogSummary, CatalogLoadError> {
    doc.validate()?;
    let contract = catalog_contract()?;
    let manual_ids: HashSet<&str> = contract
        .manual_provider_ids
        .iter()
        .map(String::as_str)
        .collect();
    let manual_provider_count = doc
        .providers
        .iter()
        .filter(|provider| manual_ids.contains(provider.stable_id()))
        .count();
    Ok(CatalogSummary {
        schema_version: doc.schema_version,
        provider_count: doc.providers.len(),
        base_provider_count: doc.providers.len().saturating_sub(manual_provider_count),
        manual_provider_count,
        model_count: doc
            .providers
            .iter()
            .map(|provider| provider.user_facing_models().len())
            .sum(),
    })
}

pub(crate) fn load_catalog_document_from_path(
    path: &Path,
) -> Result<(CatalogDoc, PathBuf), CatalogLoadError> {
    let content = std::fs::read_to_string(path).map_err(|error| CatalogLoadError::Read {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let doc = serde_json::from_str::<CatalogDoc>(&content).map_err(|error| {
        CatalogLoadError::JsonParse {
            path: path.to_path_buf(),
            line: error.line(),
            column: error.column(),
            detail: error.to_string(),
        }
    })?;
    doc.validate_at(path)?;
    Ok((doc, path.to_path_buf()))
}

/// Carga tipada sobre la resolución productiva.
///
/// Son dos capas y las dos hacen falta. La resolución (live XDG válido →
/// snapshot previo → base instalada, nunca checkout-relative) decide QUÉ
/// archivo es el catálogo: sin ella el panel lee uno que no es el que
/// `reconcile` publica. La validación tipada decide si ese archivo SIRVE: sin
/// ella un catálogo inválido degrada a un picker vacío fabricado en vez de
/// decir qué pasó y dónde.
pub(crate) fn load_catalog_document() -> Result<(CatalogDoc, PathBuf), CatalogLoadError> {
    let path = resolve_catalog_path().path;
    load_catalog_document_from_path(&path)
}

// ─── Status classification (mirrors the Python catalog's status labels) ───────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogStatus {
    UsableNow,
    Connected,
    DetectedLogin,
    RequiresApiKey,
    RequiresOauth,
    RequiresAdapter,
    RequiresLocalServer,
    NotConfigured,
    Error,
    // ADR-044 v2 bridge / detection statuses.
    BridgeNotInstalled,
    BridgeNotRunning,
    BridgeNotActive,
    BridgeManagementLocked,
    NativeLoginDetected,
    Expired,
    ProbePending,
    ProbeFailed,
    MimoDifferentFormat,
    AllModelsReserved,
    /// Cuenta puenteada y autenticada, pero el plan alcanzó su límite de uso
    /// (temporal, upstream). Re-loguear NO lo arregla.
    RateLimited,
    Unknown,
}

impl CatalogStatus {
    pub(crate) fn from_str(s: &str) -> Self {
        match s {
            "usable_now" | "usable" => Self::UsableNow,
            "connected" => Self::Connected,
            "detected_login" => Self::DetectedLogin,
            "requires_api_key" => Self::RequiresApiKey,
            "requires_oauth" => Self::RequiresOauth,
            "requires_adapter" => Self::RequiresAdapter,
            "requires_local_server" => Self::RequiresLocalServer,
            "not_configured" | "not_installed" => Self::NotConfigured,
            "error" => Self::Error,
            // v2 bridge statuses.
            "bridge_not_installed" => Self::BridgeNotInstalled,
            "bridge_not_running" => Self::BridgeNotRunning,
            "bridge_not_active" => Self::BridgeNotActive,
            "bridge_management_locked" => Self::BridgeManagementLocked,
            "native_login_detected" => Self::NativeLoginDetected,
            "expired" => Self::Expired,
            "probe_pending" => Self::ProbePending,
            "probe_failed" => Self::ProbeFailed,
            "mimo_detected_different_format" => Self::MimoDifferentFormat,
            "all_models_reserved" => Self::AllModelsReserved,
            "rate_limited" => Self::RateLimited,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn glyph(&self) -> &'static str {
        match self {
            Self::UsableNow => "✓",
            Self::Connected => "●",
            Self::DetectedLogin
            | Self::NativeLoginDetected
            | Self::BridgeNotInstalled
            | Self::BridgeNotRunning
            | Self::BridgeNotActive
            | Self::BridgeManagementLocked
            | Self::Expired
            | Self::ProbePending
            | Self::ProbeFailed
            | Self::MimoDifferentFormat
            | Self::RateLimited => "◐",
            Self::Error => "✗",
            _ => "○",
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::UsableNow => "Usable ahora",
            Self::Connected => "Conectado · probe pendiente",
            Self::DetectedLogin | Self::NativeLoginDetected => "Login nativo detectado",
            Self::BridgeNotInstalled => "Requiere instalar CLIProxyAPI",
            Self::BridgeNotRunning => "CLIProxyAPI no corre",
            Self::BridgeNotActive => "Puente no activado · conectar puente",
            Self::BridgeManagementLocked => "Management key requerida",
            Self::Expired => "Token vencido · reconectar puente",
            Self::ProbePending => "Probe de modelos pendiente",
            Self::ProbeFailed => "Probe falló · reintentar",
            Self::MimoDifferentFormat => "Formato auth diferente (SQLite)",
            Self::AllModelsReserved => "Todos los modelos reservados",
            Self::RateLimited => "Límite de uso del plan · se recupera solo",
            Self::RequiresApiKey => "API key requerida",
            Self::RequiresOauth => "OAuth requerido",
            Self::RequiresAdapter => "Adapter futuro",
            Self::RequiresLocalServer => "Requiere server local",
            Self::NotConfigured => "No configurado",
            Self::Error => "Error",
            Self::Unknown => "Desconocido",
        }
    }
}

/// Section title for provider availability. Supported entries are retained even
/// when no local source can prove them usable yet.
/// Tier de la sección "Catálogo". La fila y su header DEBEN usar el mismo
/// valor: cuando divergían, `move_section` saltaba a una sección inexistente.
pub(crate) const TIER_CATALOG: u8 = 5;

pub(crate) fn tier_title(tier: u8) -> &'static str {
    match tier {
        0 => "Disponibles ahora",
        // Sección propia: a estos NO les falta login, les falta crédito (o una
        // verificación que no se pudo completar). Mezclarlos con "detectados
        // pero incompletos" hacía que el panel mintiera por omisión.
        1 => "Credencial presente · no utilizable",
        2 => "Detectados pero incompletos",
        3 => "Soportados no configurados",
        4 => "No saludables",
        _ => "Catálogo",
    }
}

/// Estados de credencial que publica el catálogo tras verificar contra el
/// proveedor. `verified_no_credit` es una credencial VÁLIDA sin saldo: el
/// proveedor sólo responde eso a una key que reconoció.
pub(crate) const CREDENTIAL_PRESENT_NOT_USABLE: &[&str] = &[
    "verified_no_credit",
    "present_unverified",
    "invalid",
    // DESACTUALIZADO hasta el 2026-08-01: decía que el runtime "todavía no
    // sabe hablar sin Authorization". Sí sabe — LlmProvider::from_config y
    // from_config_for_alias construyen el provider sin credencial cuando el
    // catálogo declara free_access, y un chat real por deepseek-v4-flash-free
    // lo confirmó. Es la sexta vez en este proyecto que un comentario describe
    // un estado del mundo que ya cambió.
    //
    // Sigue en esta lista a propósito, pero por OTRA razón: `free_access` es
    // una credencial presente-y-no-utilizable en el sentido del panel —no hay
    // secreto que mostrar ni que rotar—, así que la sección honesta es ésta y
    // no la de "disponible con credencial".
    "free_access",
    // Cuota mensual agotada en una suscripción ACTIVA: se destraba solo en una
    // fecha conocida, que el detalle publica. No es "sin saldo".
    "quota_exhausted",
    // Hay credencial pero no se pudo determinar qué modelos habilita.
    "unknown_availability",
];

// ─── Flat navigable row model ─────────────────────────────────────────────────

/// A renderable row. Headers are non-selectable; provider/catalog rows are
/// selectable.
#[derive(Debug, Clone)]
pub(crate) enum Row {
    /// Section header ("Tus proveedores" / "Catálogo"). Not selectable.
    Header { tier: u8 },
    /// A detected provider entry ("Tus proveedores"). Selectable.
    Provider {
        tier: u8,
        entry: Box<CatalogProviderEntry>,
    },
    /// A pre-configured Catálogo provider (one-step login). Selectable.
    CatalogEntry { entry: CatalogLoginEntry },
}

impl Row {
    pub(crate) fn is_selectable(&self) -> bool {
        matches!(self, Row::Provider { .. } | Row::CatalogEntry { .. })
    }

    pub(crate) fn tier(&self) -> u8 {
        match self {
            Row::Header { tier } => *tier,
            Row::Provider { tier, .. } => *tier,
            Row::CatalogEntry { .. } => TIER_CATALOG,
        }
    }

    pub(crate) fn provider_id(&self) -> Option<&str> {
        match self {
            Row::Provider { entry, .. } => Some(&entry.id),
            Row::CatalogEntry { entry } => Some(&entry.provider_id),
            _ => None,
        }
    }
}

// ─── One-step login input (Catálogo → API key) ───────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputFocus {
    BaseUrl,
    Key,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum InputPhase {
    /// Typing the key (and base URL for region-specific providers).
    Editing,
    /// Probe subprocess running (spinner + timer in the UI).
    Validating,
    /// Network/timeout failure — offer retry (r) or cancel (Esc).
    NetworkError(String),
}

/// State of the API-key input modal for a Catálogo provider.
#[derive(Debug, Clone)]
pub(crate) struct CatalogInput {
    pub entry: CatalogLoginEntry,
    pub base_url_buf: String,
    pub key_buf: String,
    pub focus: InputFocus,
    pub phase: InputPhase,
    /// Inline error shown while Editing (e.g. "Key inválida o sin permisos").
    pub error: Option<String>,
    /// When the running validation started (for the visible timer).
    pub started: Option<Instant>,
}

impl CatalogInput {
    fn new(entry: CatalogLoginEntry) -> Self {
        let focus = if entry.needs_base_url {
            InputFocus::BaseUrl
        } else {
            InputFocus::Key
        };
        Self {
            entry,
            base_url_buf: String::new(),
            key_buf: String::new(),
            focus,
            phase: InputPhase::Editing,
            error: None,
            started: None,
        }
    }
}

/// Result of the Python one-step login subprocess.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LoginOutcome {
    Success {
        provider_id: String,
        display_name: String,
        base_url: String,
        protocol: String,
        models: Vec<String>,
    },
    /// 401/403 — bad key. Nothing was stored.
    InvalidKey { message: String },
    /// Network / timeout. Nothing was stored. Retryable.
    Network { message: String },
    /// Other failure (bad input, unexpected). Nothing was stored.
    Failed { message: String },
}

/// Data the caller (render layer) needs to persist a successful login into
/// NexumConfig. The raw key is handed over exactly once.
#[derive(Debug)]
pub(crate) struct LoginSuccess {
    pub provider_id: String,
    pub display_name: String,
    pub base_url: String,
    pub protocol: String,
    pub models: Vec<String>,
    pub api_key: String,
}

/// Maximum seconds we wait for the login subprocess before declaring a
/// network timeout (the Python probe itself times out at 5s).
const LOGIN_SUBPROCESS_TIMEOUT_SECS: u64 = 30;

// ─── Bridge jobs (Sprint C): refresh `r` + conectar puente (OAuth explícito) ──

/// Resultado de un job de puente corriendo en background.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BridgeJobOutcome {
    /// Refresh (supervisor --ensure + regen del catálogo) terminado.
    RefreshDone(Result<(), String>),
    /// Flujo "conectar puente" terminado (OAuth autorizado o error). En Ok
    /// ya se corrió el refresh, así que solo falta recargar el panel.
    /// `callback_diag` lleva el diagnóstico del callback OAuth (host/port/path
    /// sanitizados + si el listener estaba activo) para UX de fallo clara.
    ConnectDone {
        family: String,
        result: Result<(), String>,
        callback_diag: CallbackDiag,
    },
}

/// Diagnóstico del callback OAuth (sanitizado: solo host/port/path, nunca
/// code/token/state). Se completa desde `connect_url` (parseo) + probe de
/// listener, para mostrar UX de fallo útil si el login cae con
/// ERR_CONNECTION_REFUSED.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct CallbackDiag {
    /// Host del callback esperado (ej. "localhost"), None si desconocido.
    pub host: Option<String>,
    /// Puerto del callback esperado, None si desconocido.
    pub port: Option<u32>,
    /// Path del callback (ej. "/auth/callback"), None si desconocido.
    pub path: Option<String>,
    /// Origen del parseo: "parsed_from_redirect_uri" | "inferred" | "unknown".
    pub source: String,
    /// True si había un proceso escuchando en el puerto del callback.
    pub listener_detected: bool,
    /// "ipv4_only" | "ipv6_only" | "both" | "missing".
    pub listener_kind: String,
}

/// Job de puente en curso (para render/hints). El label es user-facing.
/// `cancel` permite interrumpir el poll loop del thread (Esc cancela de
/// verdad, y un nuevo OAuth del mismo provider cancela el anterior).
#[derive(Debug, Clone)]
pub(crate) struct BridgeJob {
    pub label: String,
    pub started: Instant,
    pub timeout_secs: u64,
    pub cancel: Arc<AtomicBool>,
}

impl PartialEq for BridgeJob {
    fn eq(&self, other: &Self) -> bool {
        // Cancel flag no participa en igualdad (es estado runtime, no datos).
        self.label == other.label
            && self.started == other.started
            && self.timeout_secs == other.timeout_secs
    }
}

/// Providers que se puentean vía CLIProxyAPI (OAuth explícito con Enter).
const BRIDGE_PROVIDERS: &[&str] = &["claude_code", "codex_cli", "gemini_cli"];

/// Timeout del refresh (supervisor + regen con probe online).
const REFRESH_TIMEOUT_SECS: u64 = 60;
/// Timeout de la espera de autorización OAuth en el navegador.
const CONNECT_TIMEOUT_SECS: u64 = 180;

// ─── ProviderPanel ────────────────────────────────────────────────────────────

/// Read-only, navigable provider catalog panel. Loads
/// el catálogo resuelto once on open; if missing/invalid, renders an
/// explicit error instead of any data.
#[derive(Clone)]
pub struct ProviderPanel {
    /// Parsed catalog (None if missing/invalid).
    pub catalog: Option<CatalogDoc>,
    /// Error message when the catalog is unavailable.
    pub error: Option<String>,
    /// Absolute path the panel tried to load (for the error message).
    pub catalog_path: String,
    /// Flat list of rows (headers + providers, tier-sorted).
    rows: Vec<Row>,
    /// Index into `rows` of the currently selected row. Always points at a
    /// selectable (Provider) row, or 0 if there are none.
    selected: usize,
    /// Vertical scroll offset (in rows) for long lists.
    scroll_offset: usize,
    /// Id of the provider whose detail block is expanded (Enter toggles).
    expanded_id: Option<String>,
    /// API-key input modal (Some while a Catálogo login is in progress).
    pub(crate) input: Option<CatalogInput>,
    /// Receiver for the async login subprocess result. Arc<Mutex<..>> so the
    /// panel stays Clone (PanelState requires it).
    login_rx: Option<Arc<Mutex<mpsc::Receiver<LoginOutcome>>>>,
    /// The raw key being validated — handed to the config writer on success,
    /// dropped on any failure. Never rendered, never logged.
    pending_key: Option<String>,
    /// Job de puente en curso (refresh `r` o conectar puente). Sprint C.
    pub(crate) bridge_job: Option<BridgeJob>,
    /// Receiver del resultado del job (Arc<Mutex> para mantener Clone).
    bridge_job_rx: Option<Arc<Mutex<mpsc::Receiver<BridgeJobOutcome>>>>,
    /// Último error de una acción del usuario (connect / refresh), mostrado
    /// como banner DENTRO del panel.
    ///
    /// A diferencia de `error`, que es terminal y reemplaza todo el contenido,
    /// esto no destruye el catálogo: el usuario ve la causa sin cerrar el
    /// panel. Antes el error sólo se empujaba al historial de chat, que en ese
    /// momento está tapado por el panel, y la acción se percibía como un no-op.
    pub(crate) action_error: Option<String>,
}

impl ProviderPanel {
    /// Load the catalog from the generated JSON file.
    pub fn load() -> Self {
        match load_catalog_document() {
            Ok((doc, path)) => {
                let mut panel = Self {
                    catalog: Some(doc),
                    error: None,
                    catalog_path: path.to_string_lossy().to_string(),
                    rows: Vec::new(),
                    selected: 0,
                    scroll_offset: 0,
                    expanded_id: None,
                    input: None,
                    login_rx: None,
                    pending_key: None,
                    bridge_job: None,
                    bridge_job_rx: None,
                    action_error: None,
                };
                panel.rebuild_rows();
                panel
            }
            Err(error) => {
                let path = resolve_catalog_path().path.to_string_lossy().to_string();
                Self::error_state(
                    format!(
                        "{error}\n  Diagnosticá con: nexum doctor \
                         (checks PROV-CATALOG-*)."
                    ),
                    path,
                )
            }
        }
    }

    fn error_state(msg: impl Into<String>, path_str: String) -> Self {
        Self {
            catalog: None,
            error: Some(msg.into()),
            catalog_path: path_str,
            rows: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            expanded_id: None,
            input: None,
            login_rx: None,
            pending_key: None,
            bridge_job: None,
            bridge_job_rx: None,
            action_error: None,
        }
    }

    /// Construct from an in-memory catalog document (used by tests).
    pub fn from_catalog(doc: CatalogDoc) -> Self {
        let mut panel = Self {
            catalog: Some(doc),
            error: None,
            catalog_path: "in-memory-catalog".to_string(),
            rows: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            expanded_id: None,
            input: None,
            login_rx: None,
            pending_key: None,
            bridge_job: None,
            bridge_job_rx: None,
            action_error: None,
        };
        panel.rebuild_rows();
        panel
    }

    /// Reload the catalog JSON from disk (after a successful one-step login),
    /// preserving nothing but the open panel itself.
    pub(crate) fn reload(&mut self) {
        *self = Self::load();
    }

    /// Construct an error-state panel (used by tests).
    pub fn from_error(msg: impl Into<String>) -> Self {
        Self::error_state(msg, "installed-layout-v1".to_string())
    }

    /// True when local evidence exists, even if it is insufficient to use it.
    fn is_detected(entry: &CatalogProviderEntry) -> bool {
        entry.is_usable()
            || entry.native_login_detected.unwrap_or(false)
            || entry.credential_detected
            || entry.auth_mode.as_deref() == Some("local_no_auth")
            || entry.connection_type == "local_no_auth"
    }

    fn is_unhealthy(entry: &CatalogProviderEntry) -> bool {
        matches!(
            CatalogStatus::from_str(&entry.status),
            CatalogStatus::Error
                | CatalogStatus::Expired
                | CatalogStatus::ProbeFailed
                | CatalogStatus::RateLimited
        )
    }

    /// Build the flat row list: "Tus proveedores" (detected, usable first) +
    /// "Catálogo" (pre-configured one-step logins, deduped against detected).
    /// Sección a la que pertenece un provider.
    ///
    /// La clave está en el tier 1: un provider cuya credencial ya fue
    /// verificada y resultó no utilizable (sin saldo, rechazada, o sin poder
    /// confirmarse) NO pertenece a "detectados pero incompletos" — a ese le
    /// falta login, y a estos no. Categorizarlos juntos era la mentira por
    /// omisión del panel.
    pub(crate) fn tier_of(entry: &CatalogProviderEntry) -> u8 {
        if entry.is_usable() {
            return 0;
        }
        if Self::is_unhealthy(entry) {
            return 4;
        }
        if entry
            .credential_state
            .as_deref()
            .is_some_and(|s| CREDENTIAL_PRESENT_NOT_USABLE.contains(&s))
        {
            return 1;
        }
        if Self::is_detected(entry) {
            return 2;
        }
        3
    }

    fn rebuild_rows(&mut self) {
        let mut rows: Vec<Row> = Vec::new();
        if let Some(doc) = &self.catalog {
            let norm = |s: &str| s.replace('-', "_").to_lowercase();

            let detected_ids: std::collections::HashSet<String> = doc
                .providers
                .iter()
                .flat_map(|p| {
                    [
                        norm(&p.id),
                        p.provider_id.as_deref().map(norm).unwrap_or_default(),
                    ]
                })
                .filter(|s| !s.is_empty())
                .collect();

            for tier in 0..=4 {
                let providers: Vec<CatalogProviderEntry> = doc
                    .providers
                    .iter()
                    .filter(|entry| Self::tier_of(entry) == tier)
                    .cloned()
                    .collect();
                if !providers.is_empty() {
                    rows.push(Row::Header { tier });
                    rows.extend(
                        providers
                            .into_iter()
                            .map(|entry| Row::Provider { tier, entry: Box::new(entry) }),
                    );
                }
            }

            // Additional login entries remain deduped by stable provider ID.
            let catalog_rows: Vec<CatalogLoginEntry> = doc
                .catalog
                .iter()
                .filter(|c| !detected_ids.contains(&norm(&c.provider_id)))
                .cloned()
                .collect();
            if !catalog_rows.is_empty() {
                rows.push(Row::Header { tier: TIER_CATALOG });
                for entry in catalog_rows {
                    rows.push(Row::CatalogEntry { entry });
                }
            }
        }
        self.rows = rows;
        // Snap selection to the first selectable row.
        self.selected = self
            .rows
            .iter()
            .position(|r| r.is_selectable())
            .unwrap_or(0);
        self.scroll_offset = 0;
    }

    // ── Accessors (used by the renderer + tests) ──

    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    pub(crate) fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub(crate) fn expanded_id(&self) -> Option<&str> {
        self.expanded_id.as_deref()
    }

    /// Number of selectable (provider) rows.
    pub(crate) fn selectable_count(&self) -> usize {
        self.rows.iter().filter(|r| r.is_selectable()).count()
    }

    /// The currently selected provider entry, if any.
    pub(crate) fn selected_entry(&self) -> Option<&CatalogProviderEntry> {
        self.rows.get(self.selected).and_then(|r| match r {
            Row::Provider { entry, .. } => Some(entry.as_ref()),
            _ => None,
        })
    }

    // ── Navigation ──

    /// Move selection by `delta` selectable rows (skipping headers). Clamps.
    fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        // Collect indices of selectable rows.
        let selectable: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.is_selectable())
            .map(|(i, _)| i)
            .collect();
        if selectable.is_empty() {
            return;
        }
        // Current position within the selectable list (clamped if stale).
        let cur_pos = selectable
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0);
        let new_pos = (cur_pos as i32 + delta).clamp(0, selectable.len() as i32 - 1) as usize;
        self.selected = selectable[new_pos];
    }

    /// Jump to the first selectable row of the next (`+1`) or previous (`-1`)
    /// tier relative to the selected row's tier. Used by Tab/Shift+Tab.
    fn move_section(&mut self, direction: i32) {
        let Some(cur_tier) = self.rows.get(self.selected).map(Row::tier) else {
            return;
        };
        let tiers_present: Vec<u8> = {
            let mut t: Vec<u8> = self.rows.iter().map(Row::tier).collect();
            t.sort_unstable();
            t.dedup();
            t
        };
        let cur_idx = tiers_present
            .iter()
            .position(|&t| t == cur_tier)
            .unwrap_or(0);
        let target_idx =
            (cur_idx as i32 + direction).clamp(0, tiers_present.len() as i32 - 1) as usize;
        let target_tier = tiers_present[target_idx];
        if let Some(pos) = self
            .rows
            .iter()
            .position(|r| r.tier() == target_tier && r.is_selectable())
        {
            self.selected = pos;
        }
    }

    /// El render actualiza el offset del viewport (sigue al cursor, Bug 1).
    /// (Nombre distinto de PanelComponent::set_scroll_offset(u16), que es el
    /// hook de arrastre de scrollbar.)
    pub(crate) fn set_viewport_offset(&mut self, offset: usize) {
        self.scroll_offset = offset;
    }

    /// Toggle the expanded detail for the currently selected provider.
    fn toggle_expand(&mut self) {
        if let Some(entry) = self.selected_entry() {
            let id = entry.id.clone();
            if self.expanded_id.as_deref() == Some(id.as_str()) {
                self.expanded_id = None;
            } else {
                self.expanded_id = Some(id);
            }
        }
    }

    // ── One-step login input (Catálogo) ──────────────────────────────────────

    /// The currently selected Catálogo entry, if the cursor is on one.
    pub(crate) fn selected_catalog_entry(&self) -> Option<&CatalogLoginEntry> {
        self.rows.get(self.selected).and_then(|r| match r {
            Row::CatalogEntry { entry } => Some(entry),
            _ => None,
        })
    }

    /// Open the API-key input for the selected Catálogo row (Enter).
    pub(crate) fn open_input_for_selected(&mut self) -> bool {
        if let Some(entry) = self.selected_catalog_entry().cloned() {
            self.input = Some(CatalogInput::new(entry));
            true
        } else {
            false
        }
    }

    /// Close the input modal discarding everything (Esc). Nothing persists.
    pub(crate) fn close_input(&mut self) {
        self.input = None;
        self.login_rx = None;
        self.pending_key = None;
    }

    pub(crate) fn input_push_char(&mut self, c: char) {
        if let Some(input) = &mut self.input {
            if input.phase != InputPhase::Editing {
                return;
            }
            match input.focus {
                InputFocus::BaseUrl => input.base_url_buf.push(c),
                InputFocus::Key => input.key_buf.push(c),
            }
        }
    }

    pub(crate) fn input_push_str(&mut self, s: &str) {
        // Paste path: keep only the first line, trimmed (keys never span lines).
        let cleaned: String = s
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        if let Some(input) = &mut self.input {
            if input.phase != InputPhase::Editing {
                return;
            }
            match input.focus {
                InputFocus::BaseUrl => input.base_url_buf.push_str(&cleaned),
                InputFocus::Key => input.key_buf.push_str(&cleaned),
            }
        }
    }

    pub(crate) fn input_backspace(&mut self) {
        if let Some(input) = &mut self.input {
            if input.phase != InputPhase::Editing {
                return;
            }
            match input.focus {
                InputFocus::BaseUrl => {
                    input.base_url_buf.pop();
                }
                InputFocus::Key => {
                    input.key_buf.pop();
                }
            }
        }
    }

    /// Tab/↑/↓ inside the modal: toggle field focus (dual-field providers only).
    pub(crate) fn input_toggle_focus(&mut self) {
        if let Some(input) = &mut self.input {
            if input.entry.needs_base_url && input.phase == InputPhase::Editing {
                input.focus = match input.focus {
                    InputFocus::BaseUrl => InputFocus::Key,
                    InputFocus::Key => InputFocus::BaseUrl,
                };
            }
        }
    }

    /// Validate buffers and move to Validating, returning the request to spawn
    /// (provider_id, api_key, base_url). Pure state logic — the caller spawns
    /// the subprocess (tests inject outcomes via `apply_outcome`).
    pub(crate) fn begin_validation(&mut self) -> Option<(String, String, String)> {
        let input = self.input.as_mut()?;
        if input.phase == InputPhase::Validating {
            return None;
        }
        let key = input.key_buf.trim().to_string();
        if key.is_empty() {
            input.error = Some("Ingresá una API key.".to_string());
            input.phase = InputPhase::Editing;
            return None;
        }
        let base_url = input.base_url_buf.trim().trim_end_matches('/').to_string();
        if input.entry.needs_base_url && !base_url.starts_with("http") {
            input.error = Some(
                "Este proveedor necesita la base URL de tu dashboard (https://…).".to_string(),
            );
            input.focus = InputFocus::BaseUrl;
            input.phase = InputPhase::Editing;
            return None;
        }
        input.error = None;
        input.phase = InputPhase::Validating;
        input.started = Some(Instant::now());
        self.pending_key = Some(key.clone());
        Some((input.entry.provider_id.clone(), key, base_url))
    }

    /// Spawn the Python one-step login subprocess for a request produced by
    /// `begin_validation`. The result arrives via `poll_login_result`.
    pub(crate) fn spawn_login(&mut self, provider_id: String, api_key: String, base_url: String) {
        let (tx, rx) = mpsc::channel();
        self.login_rx = Some(Arc::new(Mutex::new(rx)));
        let catalog_path = self.catalog_path.clone();
        std::thread::spawn(move || {
            let outcome = run_login_subprocess(&provider_id, &api_key, &base_url, &catalog_path);
            let _ = tx.send(outcome);
        });
    }

    /// Non-blocking poll of the login subprocess. Also enforces the UI-side
    /// timeout so the modal never hangs in "validando…".
    pub(crate) fn poll_login_result(&mut self) -> Option<LoginOutcome> {
        let rx = self.login_rx.as_ref()?;
        let polled = rx.lock().ok().and_then(|r| r.try_recv().ok());
        if let Some(outcome) = polled {
            self.login_rx = None;
            return Some(outcome);
        }
        // Timeout guard.
        let timed_out = self
            .input
            .as_ref()
            .and_then(|i| i.started)
            .is_some_and(|t| t.elapsed().as_secs() >= LOGIN_SUBPROCESS_TIMEOUT_SECS);
        if timed_out {
            self.login_rx = None;
            return Some(LoginOutcome::Network {
                message: "Timeout esperando la validación.".to_string(),
            });
        }
        None
    }

    /// Whether a validation is currently in flight (drives forced redraws).
    pub(crate) fn is_validating(&self) -> bool {
        matches!(
            self.input.as_ref().map(|i| &i.phase),
            Some(InputPhase::Validating)
        )
    }

    /// Apply a login outcome to the panel state. On success returns the data
    /// needed to persist the provider into NexumConfig (with the raw key,
    /// handed over exactly once). On failure nothing is persisted.
    pub(crate) fn apply_outcome(&mut self, outcome: LoginOutcome) -> Option<LoginSuccess> {
        match outcome {
            LoginOutcome::Success {
                provider_id,
                display_name,
                base_url,
                protocol,
                models,
            } => {
                let api_key = self.pending_key.take()?;
                self.input = None;
                self.login_rx = None;
                Some(LoginSuccess {
                    provider_id,
                    display_name,
                    base_url,
                    protocol,
                    models,
                    api_key,
                })
            }
            LoginOutcome::InvalidKey { message } => {
                self.pending_key = None;
                if let Some(input) = &mut self.input {
                    // Reopen the input with the previous value cleared.
                    input.key_buf.clear();
                    input.focus = InputFocus::Key;
                    input.phase = InputPhase::Editing;
                    input.error = Some(message);
                    input.started = None;
                }
                None
            }
            LoginOutcome::Network { message } => {
                self.pending_key = None;
                if let Some(input) = &mut self.input {
                    input.phase = InputPhase::NetworkError(message);
                    input.started = None;
                }
                None
            }
            LoginOutcome::Failed { message } => {
                self.pending_key = None;
                if let Some(input) = &mut self.input {
                    input.phase = InputPhase::Editing;
                    input.error = Some(message);
                    input.started = None;
                }
                None
            }
        }
    }

    // ── Bridge jobs (Sprint C): refresh `r` + conectar puente ────────────────

    /// True si la fila es un provider puenteado por CLIProxyAPI.
    /// Providers cuyo `connect` abre el flujo OAuth del puente.
    ///
    /// Derivado del catálogo (`connect_kind`, que sale del `auth_mode` del
    /// registry). La lista fija de 3 ids queda sólo como respaldo para
    /// catálogos viejos que todavía no publican el campo.
    pub(crate) fn is_bridge_provider(entry: &CatalogProviderEntry) -> bool {
        if let Some(kind) = entry.connect_kind.as_deref() {
            return kind == "bridge_oauth";
        }
        if entry.auth_mode.as_deref() == Some("cli_oauth") {
            return true;
        }
        let norm = |s: &str| s.replace('-', "_").to_lowercase();
        let pid = entry
            .provider_id
            .clone()
            .unwrap_or_else(|| entry.id.clone());
        BRIDGE_PROVIDERS.contains(&norm(&pid).as_str())
    }

    /// Providers cuya credencial se lee de un almacén en disco: el `connect`
    /// no abre OAuth, explica de dónde se lee y cómo re-loguearse con su CLI.
    pub(crate) fn is_credential_store_provider(entry: &CatalogProviderEntry) -> bool {
        entry.connect_kind.as_deref() == Some("credential_store")
    }

    /// Por qué este provider no ofrece un flujo de conexión. Se muestra cuando
    /// Enter no tiene nada que lanzar, para que la ausencia de acción sea una
    /// respuesta y no un silencio indistinguible de un cuelgue.
    pub(crate) fn no_flow_reason(entry: &CatalogProviderEntry) -> String {
        let nombre = &entry.display_name;
        match entry.connect_kind.as_deref() {
            Some("api_key_input") => format!(
                "{nombre} — sin credencial en el sistema.\n  \
                 Remedio: pegá una API key desde la sección «Catálogo»."
            ),
            Some("bridge_oauth") => format!(
                "{nombre} — hay un flujo de puente disponible pero no se pudo iniciar.\n  \
                 Remedio: verificá el puente con `r` y reintentá."
            ),
            _ => format!(
                "{nombre} — este provider no expone un flujo de conexión.\n  \
                 Remedio: revisá su estado con Espacio (detalle) o refrescá con `r`."
            ),
        }
    }

    /// Mensaje accionable para un provider de almacén: qué se leyó, en qué
    /// estado quedó y qué hacer al respecto. Nunca queda en silencio.
    pub(crate) fn credential_store_hint(entry: &CatalogProviderEntry) -> String {
        let mut lines = vec![format!("{} — credencial leída del disco", entry.display_name)];
        if let Some(store) = entry.credential_store.as_deref() {
            lines.push(format!("  Almacén: {store}"));
        }
        if let Some(detail) = entry.credential_detail.as_deref() {
            lines.push(format!("  Estado:  {detail}"));
        }
        match entry.relogin_command.as_deref() {
            Some(cmd) => lines.push(format!("  Remedio: reautenticá con `{cmd}` y refrescá con `r`.")),
            None => lines.push("  Remedio: refrescá con `r` tras reautenticar en su CLI.".into()),
        }
        lines.join("\n")
    }

    /// Arranca el refresh manual (`r`): supervisor --ensure + regen del
    /// catálogo con probe online, en background.
    pub(crate) fn start_refresh(&mut self) {
        if self.bridge_job.is_some() {
            return;
        }
        self.clear_action_error();
        let (tx, rx) = mpsc::channel();
        self.bridge_job_rx = Some(Arc::new(Mutex::new(rx)));
        self.bridge_job = Some(BridgeJob {
            label: "Refrescando puente y catálogo…".to_string(),
            started: Instant::now(),
            timeout_secs: REFRESH_TIMEOUT_SECS,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        let catalog_path = self.catalog_path.clone();
        std::thread::spawn(move || {
            let result = run_bridge_refresh(&catalog_path);
            let _ = tx.send(BridgeJobOutcome::RefreshDone(result));
        });
    }

    /// Arranca el flujo EXPLÍCITO "conectar puente" (Enter sobre
    /// Claude/Codex/Gemini no usable): pide la URL OAuth, abre el navegador
    /// y espera la autorización del usuario; al confirmar, refresca.
    ///
    /// Si ya hay un job en curso (ej. un OAuth anterior del mismo provider),
    /// se lo cancela primero: setea su flag `cancel` para que el poll loop
    /// del thread salga limpio, y arranca el nuevo.
    /// Limpia el banner de error del panel (una acción nueva empieza limpia).
    pub(crate) fn clear_action_error(&mut self) {
        self.action_error = None;
    }

    /// Publica el error de una acción DENTRO del panel, sin destruir el catálogo.
    pub(crate) fn set_action_error(&mut self, msg: impl Into<String>) {
        self.action_error = Some(msg.into());
    }

    pub(crate) fn start_connect(&mut self, entry: &CatalogProviderEntry) {
        self.clear_action_error();
        // Nuevo-OAuth-cancelea-anterior: si hay un job vivo, señal de cancel.
        if let Some(job) = self.bridge_job.take() {
            job.cancel.store(true, Ordering::SeqCst);
        }
        let pid = entry
            .provider_id
            .clone()
            .unwrap_or_else(|| entry.id.clone())
            .replace('-', "_");
        let family = entry.family_or_name().to_string();
        let (tx, rx) = mpsc::channel();
        self.bridge_job_rx = Some(Arc::new(Mutex::new(rx)));
        let cancel = Arc::new(AtomicBool::new(false));
        self.bridge_job = Some(BridgeJob {
            label: format!("Autorizá {family} en el navegador…"),
            started: Instant::now(),
            timeout_secs: CONNECT_TIMEOUT_SECS,
            cancel: cancel.clone(),
        });
        let catalog_path = self.catalog_path.clone();
        std::thread::spawn(move || {
            let (result, diag) = run_bridge_connect(&pid, &catalog_path, &cancel);
            let _ = tx.send(BridgeJobOutcome::ConnectDone {
                family,
                result,
                callback_diag: diag,
            });
        });
    }

    /// Cancela el job en curso (Esc). Setea el flag `cancel` para que el
    /// poll loop del thread salga en ≤0.2s (cancelación real, no solo
    /// descarte del resultado).
    pub(crate) fn cancel_bridge_job(&mut self) {
        if let Some(job) = self.bridge_job.take() {
            job.cancel.store(true, Ordering::SeqCst);
        }
        self.bridge_job_rx = None;
    }

    /// Poll no bloqueante del job (con timeout defensivo).
    pub(crate) fn poll_bridge_job(&mut self) -> Option<BridgeJobOutcome> {
        let job = self.bridge_job.as_ref()?;
        let timed_out = job.started.elapsed().as_secs() >= job.timeout_secs;
        let rx = self.bridge_job_rx.as_ref()?;
        let polled = rx.lock().ok().and_then(|r| r.try_recv().ok());
        if let Some(outcome) = polled {
            self.bridge_job = None;
            self.bridge_job_rx = None;
            return Some(outcome);
        }
        if timed_out {
            self.bridge_job = None;
            self.bridge_job_rx = None;
            return Some(BridgeJobOutcome::RefreshDone(Err(
                "Timeout esperando el job del puente.".to_string(),
            )));
        }
        None
    }

    /// Key handling while the API-key modal is open. Every key is Consumed.
    fn handle_input_key(&mut self, input: Input, ctx: &mut PanelContext<'_>) -> EventResult {
        use tui_textarea::Key;
        let phase = match self.input.as_ref() {
            Some(i) => i.phase.clone(),
            None => return EventResult::Consumed,
        };
        match phase {
            InputPhase::Validating => match input {
                Input { key: Key::Esc, .. } => {
                    // Cancel: nothing persists; the detached thread's result is
                    // discarded (login_rx dropped).
                    ctx.session_mgr.current_mut().ui.loading = false;
                    self.close_input();
                    EventResult::Consumed
                }
                _ => EventResult::Consumed,
            },
            InputPhase::NetworkError(_) => match input {
                Input { key: Key::Esc, .. } => {
                    self.close_input();
                    EventResult::Consumed
                }
                Input {
                    key: Key::Char('r'),
                    ctrl: false,
                    alt: false,
                    ..
                } => {
                    if let Some(i) = &mut self.input {
                        i.phase = InputPhase::Editing;
                    }
                    if let Some((pid, key, base)) = self.begin_validation() {
                        ctx.session_mgr.current_mut().ui.loading = true;
                        self.spawn_login(pid, key, base);
                    }
                    EventResult::Consumed
                }
                _ => EventResult::Consumed,
            },
            InputPhase::Editing => match input {
                Input { key: Key::Esc, .. } => {
                    self.close_input();
                    EventResult::Consumed
                }
                Input {
                    key: Key::Enter, ..
                } => {
                    let advance_field = self
                        .input
                        .as_ref()
                        .is_some_and(|i| i.entry.needs_base_url && i.focus == InputFocus::BaseUrl);
                    if advance_field {
                        self.input_toggle_focus();
                    } else if let Some((pid, key, base)) = self.begin_validation() {
                        ctx.session_mgr.current_mut().ui.loading = true;
                        self.spawn_login(pid, key, base);
                    }
                    EventResult::Consumed
                }
                Input { key: Key::Tab, .. }
                | Input { key: Key::Up, .. }
                | Input { key: Key::Down, .. } => {
                    self.input_toggle_focus();
                    EventResult::Consumed
                }
                Input {
                    key: Key::Backspace,
                    ..
                } => {
                    self.input_backspace();
                    EventResult::Consumed
                }
                Input {
                    key: Key::Char(c),
                    ctrl: false,
                    alt: false,
                    ..
                } => {
                    self.input_push_char(c);
                    EventResult::Consumed
                }
                _ => EventResult::Consumed,
            },
        }
    }
}

/// Run the Python one-step login (`src/nexum_providers/provider_login.py`).
///
/// The request travels as JSON via stdin (the key is NEVER in argv/env). The
/// result comes back as one JSON line on stdout. This mirrors the mechanism
/// the panel already uses for the catalog: plain files/JSON + the Python layer
/// as the single source of truth.
fn run_login_subprocess(
    provider_id: &str,
    api_key: &str,
    base_url: &str,
    catalog_path: &str,
) -> LoginOutcome {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let script = provider_resource_root()
        .map(|root| root.join("src/nexum_providers/provider_login.py"))
        .filter(|path| path.exists());
    let Some(script) = script else {
        return LoginOutcome::Failed {
            message: "No se encontró el validador de provider instalado.".to_string(),
        };
    };

    let request = serde_json::json!({
        "provider_id": provider_id,
        "api_key": api_key,
        "base_url": base_url,
    });

    let child = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(&script)
        .arg("--catalog")
        .arg(catalog_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return LoginOutcome::Failed {
                message: format!("No se pudo lanzar python3: {e}"),
            }
        }
    };
    if let Some(stdin) = child.stdin.take() {
        let mut stdin = stdin;
        if stdin.write_all(request.to_string().as_bytes()).is_err() {
            let _ = child.kill();
            return LoginOutcome::Failed {
                message: "No se pudo enviar la request al validador.".to_string(),
            };
        }
    }
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            return LoginOutcome::Failed {
                message: format!("Validador falló: {e}"),
            }
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let parsed: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => {
            return LoginOutcome::Failed {
                message: "Respuesta inválida del validador.".to_string(),
            }
        }
    };
    if parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let get = |k: &str| {
            parsed
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let models = parsed
            .get("models")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        LoginOutcome::Success {
            provider_id: get("provider_id"),
            display_name: get("display_name"),
            base_url: get("base_url"),
            protocol: get("protocol"),
            models,
        }
    } else {
        let message = parsed
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Falló la validación.")
            .to_string();
        match parsed.get("error_kind").and_then(|v| v.as_str()) {
            Some("invalid_key") => LoginOutcome::InvalidKey {
                message: "Key inválida o sin permisos".to_string(),
            },
            Some("network") => LoginOutcome::Network { message },
            _ => LoginOutcome::Failed { message },
        }
    }
}

// ─── Bridge job subprocess helpers (Sprint C) ────────────────────────────────

/// Recursos de providers empaquetados. No existe fallback implícito al checkout.
fn provider_resource_root_with(module: &str) -> Option<std::path::PathBuf> {
    provider_resource_root().filter(|root| root.join(module).exists())
}

/// Corre el bridge_supervisor con los argumentos dados y parsea el JSON de
/// stdout. Nunca loguea contenido de keys (el supervisor tampoco las emite).
fn run_supervisor(
    resource_root: &std::path::Path,
    args: &[&str],
) -> Result<serde_json::Value, String> {
    use std::process::{Command, Stdio};
    let script = resource_root.join("src/nexum_providers/bridge_supervisor.py");
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(&script)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("No se pudo lanzar python3: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    serde_json::from_str(line).map_err(|_| "Respuesta inválida del supervisor.".to_string())
}

/// Regenera el catálogo con probe online. IMPORTANTE: sin el XDG aislado del
/// launcher — `opencode models` necesita su auth real en ~/.local/share.
fn run_catalog_regen(resource_root: &std::path::Path) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let script = resource_root.join("src/nexum_providers/catalog_gen/generator.py");
    let status = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(&script)
        .arg("--probe-online")
        .current_dir(resource_root)
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_CACHE_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("No se pudo regenerar el catálogo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Regen del catálogo falló (exit {status})."))
    }
}

/// Auto-login desde env vars (ej. ZAI_CODING_API_KEY → glm_coding_plan).
///
/// Best-effort y no-bloqueante: si la env var está ausente o el probe falla,
/// el arranque del TUI continúa sin problemas (el usuario puede loguear
/// manualmente vía /provedor). Solo stdout se parsea — la key viaja por env,
/// nunca por argv/stdin/stdout (el script emite únicamente el fingerprint).
fn run_auto_login(resource_root: &std::path::Path) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let script = resource_root.join("src/nexum_providers/provider_auto_login.py");
    // El script lee ZAI_CODING_API_KEY del entorno heredado (no lo pasamos
    // como arg: argv nunca debe llevar secrets). Stdin null, stderr null.
    let _status = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("No se pudo lanzar provider_auto_login: {e}"))?;
    // Exit 0 siempre (incluso en no-op/fallo): no propagamos el error.
    Ok(())
}

/// Refresh completo: supervisor --ensure + auto-login (env vars) + regen del catálogo.
fn run_bridge_refresh(_catalog_path: &str) -> Result<(), String> {
    let root = provider_resource_root_with("src/nexum_providers/bridge_supervisor.py")
        .ok_or_else(|| "No se encontró el runtime de providers instalado.".to_string())?;
    let ensure = run_supervisor(&root, &["--ensure"])?;
    // El refresh sigue aunque el puente esté caído: el catálogo refleja el
    // estado real (not_installed/start_failed) — nunca mudo.
    let _ = ensure;
    // Auto-login best-effort ANTES del regen: si ZAI_CODING_API_KEY está en el
    // entorno y probea OK, glm_coding_plan queda usable_now y el regen del
    // catálogo lo refleja (junto con glm-5.2 / glm-5-turbo en /modelo).
    // Errores no bloquean el arranque (no-op si falta la env var).
    let _ = run_auto_login(&root);
    run_catalog_regen(&root)
}

/// Flujo "conectar puente": URL OAuth → navegador → poll hasta ok/error →
/// refresh. El OAuth lo autoriza el USUARIO en el navegador (explícito).
///
/// `cancel` permite interrumpir el poll loop (Esc / nuevo OAuth del mismo
/// provider): se chequea cada 0.2s, si está seteado el flujo sale con
/// `Err("cancelado")` sin más llamadas al supervisor.
///
/// Devuelve `(result, callback_diag)`. `callback_diag` lleva el callback OAuth
/// esperado (host/port/path sanitizados + si el listener estaba activo),
/// parseado desde la auth_url por el supervisor Python, para UX de fallo.
fn run_bridge_connect(
    provider_id: &str,
    _catalog_path: &str,
    cancel: &Arc<AtomicBool>,
) -> (Result<(), String>, CallbackDiag) {
    use std::process::{Command, Stdio};
    let mut diag = CallbackDiag::default();
    let root = match provider_resource_root_with("src/nexum_providers/bridge_supervisor.py") {
        Some(r) => r,
        None => {
            return (
                Err("No se encontró el runtime de providers instalado.".into()),
                diag,
            )
        }
    };
    let resp = match run_supervisor(&root, &["--connect", provider_id]) {
        Ok(r) => r,
        Err(e) => return (Err(e), diag),
    };
    if !resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        return (
            Err(resp
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("No se pudo iniciar el flujo OAuth.")
                .to_string()),
            diag,
        );
    }
    let url = match resp.get("url").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return (Err("Flujo OAuth sin URL.".into()), diag),
    };
    let state = match resp.get("state").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return (Err("Flujo OAuth sin state.".into()), diag),
    };
    // Extraer el callback diag desde la respuesta enriquecida del supervisor
    // (host/port/path sanitizados — nunca code/token/state).
    if let Some(cb) = resp.get("callback").and_then(|v| v.as_object()) {
        diag.host = cb
            .get("host")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        diag.port = cb.get("port").and_then(|v| v.as_u64()).map(|p| p as u32);
        diag.path = cb
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        diag.source = cb
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
    }
    // Abrir el navegador (si falla, el usuario puede abrir la URL a mano;
    // la URL no es un secret — es la página de autorización del proveedor).
    let opened = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok();
    if !opened {
        return (
            Err(format!("No pude abrir el navegador. Abrí a mano: {url}")),
            diag,
        );
    }
    // Diagnóstico del listener del callback (best-effort, no fatal).
    // Solo si conocemos el puerto; si source=="unknown", no hay nada que probar.
    if let Some(port) = diag.port {
        let ldiag = probe_callback_listener(port);
        diag.listener_detected = ldiag.0;
        diag.listener_kind = ldiag.1;
    }
    // Poll cancelable: busy-poll de 0.2s × N (en vez de sleep(2s) bloqueante).
    // Cada tick chequea `cancel` → si true, sale limpio sin más supervisor calls.
    // Cada ~2s (10 ticks) hace un poll del supervisor para no saturar la API.
    const TICK: std::time::Duration = std::time::Duration::from_millis(200);
    let mut ticks_since_poll = 0u32;
    let result = loop {
        if cancel.load(Ordering::SeqCst) {
            break Err("OAuth cancelado.".to_string());
        }
        std::thread::sleep(TICK);
        ticks_since_poll += 1;
        if ticks_since_poll < 10 {
            continue;
        }
        ticks_since_poll = 0;
        match run_supervisor(&root, &["--poll", &state]) {
            Ok(poll) => match poll.get("status").and_then(|v| v.as_str()) {
                Some("ok") => break Ok(()),
                Some("wait") | None => continue,
                Some(other) => break Err(format!("El flujo OAuth terminó con estado '{other}'.")),
            },
            Err(e) => break Err(e),
        }
    };
    let result = match result {
        Ok(()) => {
            // Autorizado: refrescar para que el provider pase a usable con modelos.
            let _ = run_supervisor(&root, &["--ensure"]);
            run_catalog_regen(&root)
        }
        Err(e) => Err(e),
    };
    (result, diag)
}

/// Probe del listener del callback OAuth (TCP connect, read-only).
/// Devuelve (detected, kind) donde kind ∈ {"both","ipv4_only","ipv6_only","missing"}.
fn probe_callback_listener(port: u32) -> (bool, String) {
    use std::net::TcpStream;
    use std::net::ToSocketAddrs;
    use std::time::Duration;
    let timeout = Duration::from_millis(500);
    let probe = |host: &str| -> bool {
        let addr = format!("{host}:{port}");
        addr.to_socket_addrs()
            .ok()
            .and_then(|mut it| it.next())
            .and_then(|sa| TcpStream::connect_timeout(&sa, timeout).ok())
            .is_some()
    };
    let ipv4 = probe("127.0.0.1");
    let ipv6 = probe("::1");
    let detected = ipv4 || ipv6;
    let kind = match (ipv4, ipv6) {
        (true, true) => "both",
        (true, false) => "ipv4_only",
        (false, true) => "ipv6_only",
        (false, false) => "missing",
    }
    .to_string();
    (detected, kind)
}

fn valid_catalog(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok())
        .and_then(|doc| {
            doc.get("providers")
                .and_then(|providers| providers.as_array())
                .map(|_| ())
        })
        .is_some()
}

pub(crate) fn resolve_catalog_from_candidates(
    live: &std::path::Path,
    previous: &std::path::Path,
    base: &std::path::Path,
) -> CatalogResolution {
    let live_rejected = live.exists() && !valid_catalog(live);
    if valid_catalog(live) {
        return CatalogResolution {
            path: live.to_path_buf(),
            source: CatalogSource::Live,
            live_rejected,
        };
    }
    if valid_catalog(previous) {
        return CatalogResolution {
            path: previous.to_path_buf(),
            source: CatalogSource::Previous,
            live_rejected,
        };
    }
    if valid_catalog(base) {
        return CatalogResolution {
            path: base.to_path_buf(),
            source: CatalogSource::Base,
            live_rejected,
        };
    }
    CatalogResolution {
        path: live.to_path_buf(),
        source: CatalogSource::Missing,
        live_rejected,
    }
}

/// Resolución productiva. Delega en la FUENTE ÚNICA de `nexum-acp`.
///
/// Este módulo tenía su propia construcción de la ruta, idéntica en intención
/// pero separada en el código. La copia paralela de `nexum-acp` apuntaba a otro
/// archivo y de ahí salieron los 502. Se conserva el nombre para no tocar a los
/// llamadores; la lógica vive en un solo lugar.
fn resolve_catalog_path() -> CatalogResolution {
    nexum_acp::provider::catalog_path::resolve()
}

/// Ruta pública de resolución del catálogo (para Doctor y otros consumidores).
pub(crate) fn catalog_resolved_path() -> std::path::PathBuf {
    resolve_catalog_path().path
}

pub(crate) fn catalog_resolution() -> CatalogResolution {
    resolve_catalog_path()
}

// ─── PanelComponent ───────────────────────────────────────────────────────────

impl PanelComponent for ProviderPanel {
    fn kind(&self) -> PanelKind {
        PanelKind::Provider
    }

    fn handle_key(&mut self, input: Input, _ctx: &mut PanelContext<'_>) -> EventResult {
        use tui_textarea::Key;
        // Modal input (Catálogo login) captures ALL keys while open.
        if self.input.is_some() {
            return self.handle_input_key(input, _ctx);
        }
        // This panel captures focus: every key is Consumed (never falls through to
        // the textarea), so Tab does NOT cycle Build/Plan/Think and Enter does NOT
        // submit a prompt while the panel is open.
        match input {
            Input { key: Key::Esc, .. } => {
                if self.bridge_job.is_some() {
                    // Cancela el job del puente sin cerrar el panel.
                    self.cancel_bridge_job();
                    _ctx.session_mgr.current_mut().ui.loading = false;
                    return EventResult::Consumed;
                }
                EventResult::ClosePanel
            }
            // r: refrescar puente + catálogo (Sprint C §6).
            Input {
                key: Key::Char('r'),
                ctrl: false,
                alt: false,
                ..
            } => {
                if self.bridge_job.is_none() {
                    self.start_refresh();
                    _ctx.session_mgr.current_mut().ui.loading = true;
                }
                EventResult::Consumed
            }
            // ↑ / k: previous selectable row.
            Input { key: Key::Up, .. }
            | Input {
                key: Key::Char('k'),
                ctrl: false,
                alt: false,
                ..
            } => {
                self.move_selection(-1);
                EventResult::Consumed
            }
            // ↓ / j: next selectable row.
            Input { key: Key::Down, .. }
            | Input {
                key: Key::Char('j'),
                ctrl: false,
                alt: false,
                ..
            } => {
                self.move_selection(1);
                EventResult::Consumed
            }
            // PgUp / Ctrl+U: una página (8 ítems) hacia arriba.
            Input {
                key: Key::PageUp, ..
            }
            | Input {
                key: Key::Char('u'),
                ctrl: true,
                ..
            } => {
                self.move_selection(-8);
                EventResult::Consumed
            }
            // PgDn / Ctrl+D: una página hacia abajo.
            Input {
                key: Key::PageDown, ..
            }
            | Input {
                key: Key::Char('d'),
                ctrl: true,
                ..
            } => {
                self.move_selection(8);
                EventResult::Consumed
            }
            // g / Home: primer ítem.
            Input { key: Key::Home, .. }
            | Input {
                key: Key::Char('g'),
                ctrl: false,
                alt: false,
                ..
            } => {
                self.move_selection(i32::MIN / 2);
                EventResult::Consumed
            }
            // G / End: último ítem.
            Input { key: Key::End, .. }
            | Input {
                key: Key::Char('G'),
                ctrl: false,
                alt: false,
                ..
            } => {
                self.move_selection(i32::MAX / 2);
                EventResult::Consumed
            }
            // Shift+Tab (Tab + shift) MUST be matched before plain Tab.
            Input {
                key: Key::Tab,
                shift: true,
                ..
            } => {
                self.move_section(-1);
                EventResult::Consumed
            }
            // Tab (plain): next section.
            Input { key: Key::Tab, .. } => {
                self.move_section(1);
                EventResult::Consumed
            }
            // Space: toggle del detalle para CUALQUIER fila (los puentes no
            // usables usan Enter para conectar, así que el detalle vive acá).
            Input {
                key: Key::Char(' '),
                ..
            } => {
                self.toggle_expand();
                EventResult::Consumed
            }
            // Enter: Catálogo row → open the API-key input; provider puente
            // (Claude/Codex/Gemini) no usable → conectar puente (OAuth
            // EXPLÍCITO en el navegador, Sprint C); resto → toggle detalle.
            Input {
                key: Key::Enter, ..
            } => {
                if !self.open_input_for_selected() {
                    let selected = self.selected_entry().cloned();
                    let puede_conectar = selected.as_ref().is_some_and(|e| {
                        Self::is_bridge_provider(e) && !e.is_usable() && self.bridge_job.is_none()
                    });
                    // Provider de almacén: no hay OAuth que abrir, pero SÍ hay
                    // algo que decir. Antes caía en toggle_expand() y se veía
                    // como si Enter no hiciera nada (RC-7).
                    let hint = selected.as_ref().and_then(|e| {
                        if !puede_conectar && Self::is_credential_store_provider(e) {
                            Some(Self::credential_store_hint(e))
                        } else {
                            None
                        }
                    });
                    if puede_conectar {
                        if let Some(entry) = selected {
                            self.start_connect(&entry);
                            _ctx.session_mgr.current_mut().ui.loading = true;
                        }
                    } else if let Some(hint) = hint {
                        self.set_action_error(hint);
                    } else {
                        // Enter NUNCA queda en silencio: si no hay flujo que
                        // lanzar, se dice por qué no lo hay. El detalle se
                        // expande igual, pero el motivo es explícito.
                        self.toggle_expand();
                        if let Some(entry) = selected {
                            if !entry.is_usable() {
                                self.set_action_error(Self::no_flow_reason(&entry));
                            }
                        }
                    }
                }
                EventResult::Consumed
            }
            // Everything else is swallowed (focus capture); never reaches textarea.
            _ => EventResult::Consumed,
        }
    }

    fn desired_height(&self, _screen_height: u16, _screen_width: u16) -> u16 {
        if self.error.is_some() {
            return 12;
        }
        // ~1.5 lines per row + footer; expand adds detail height; capped.
        let base = self.rows.len() as u16 + 6;
        let expanded_extra = if self.expanded_id.is_some() { 6 } else { 0 };
        let input_extra = if self.input.is_some() { 12 } else { 0 };
        // El banner de error de acción ocupa sus propias líneas + separador.
        let action_error_extra = self
            .action_error
            .as_deref()
            .map(|msg| msg.lines().count() as u16 + 1)
            .unwrap_or(0);
        (base + expanded_extra + input_extra + action_error_extra).min(40)
    }

    fn handle_paste(&mut self, text: &str, _ctx: &mut PanelContext<'_>) -> EventResult {
        // Pasting an API key into the modal must NEVER land in the chat
        // textarea. While the modal is open, paste goes to the focused field.
        if self.input.is_some() {
            self.input_push_str(text);
            EventResult::Consumed
        } else {
            EventResult::Consumed // panel captures focus; swallow stray pastes
        }
    }

    fn render(&mut self, f: &mut Frame, app: &mut App, area: Rect) {
        crate::ui::main_ui::panels::provider::render_provider_panel(f, self, app, area);
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn status_bar_hints(&self, _lc: &crate::i18n::LcRegistry) -> Vec<(String, String)> {
        if let Some(input) = &self.input {
            return match input.phase {
                InputPhase::Validating => vec![("Esc".to_string(), "Cancelar".to_string())],
                InputPhase::NetworkError(_) => vec![
                    ("r".to_string(), "Reintentar".to_string()),
                    ("Esc".to_string(), "Cerrar".to_string()),
                ],
                InputPhase::Editing => {
                    let mut hints = vec![
                        ("Enter".to_string(), "Validar".to_string()),
                        ("Esc".to_string(), "Cancelar".to_string()),
                    ];
                    if input.entry.needs_base_url {
                        hints.insert(1, ("Tab".to_string(), "Campo".to_string()));
                    }
                    hints
                }
            };
        }
        if self.bridge_job.is_some() {
            return vec![
                ("Esc".to_string(), "Cancelar job".to_string()),
                ("↑↓/jk".to_string(), "Mover".to_string()),
            ];
        }
        vec![
            ("↑↓/jk".to_string(), "Mover".to_string()),
            ("Enter".to_string(), "Conectar".to_string()),
            ("Space".to_string(), "Detalles".to_string()),
            ("r".to_string(), "Refrescar puente".to_string()),
            ("Esc".to_string(), "Cerrar".to_string()),
        ]
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {

    /// El catálogo base que realmente se empaqueta. Los tests que lo usan
    /// verifican propiedades del artefacto, no de un fixture inventado.
    fn known_good_base_doc() -> CatalogDoc {
        serde_json::from_str(include_str!("../../../config/provider-catalog-base.json")).unwrap()
    }

    use super::*;

    #[test]
    fn real_installed_catalog_fixture_loads() {
        let content = include_str!("../../../config/provider-catalog-base.json");
        let doc = serde_json::from_str::<CatalogDoc>(content)
            .expect("candidate catalog must deserialize");
        doc.validate()
            .expect("candidate catalog must satisfy the installed contract");
    }

    #[test]
    fn installed_catalog_loads_with_real_settings_shape() {
        let doc = known_good_base_doc();
        let settings: crate::config::NexumConfig = serde_json::from_str(
            r#"{
                "$schema": "https://nexum.invalid/settings.schema.json",
                "config": {
                    "active_provider_id": "codex_cli",
                    "active_alias": "gpt-5.6-terra",
                    "providers": [{
                        "id": "codex_cli",
                        "type": "openai",
                        "apiKey": "",
                        "baseUrl": "http://127.0.0.1:8317/v1",
                        "name": "Codex / OpenAI (puente CLIProxyAPI)",
                        "models": {}
                    }]
                }
            }"#,
        )
        .expect("real settings shape");
        let provider = doc
            .provider(&settings.config.active_provider_id)
            .expect("selected provider exists");
        assert!(
            provider
                .user_facing_models()
                .contains(&settings.config.active_alias),
            "selected model exists"
        );
    }

    #[test]
    fn catalog_schema_v2_is_accepted_if_current_contract() {
        let doc = known_good_base_doc();
        assert_eq!(doc.schema_version, Some(2));
        assert!(doc.validate().is_ok());
    }

    #[test]
    fn catalog_schema_mismatch_is_structured() {
        let mut doc = known_good_base_doc();
        doc.schema_version = Some(99);
        assert!(matches!(
            doc.validate(),
            Err(CatalogLoadError::SchemaVersionMismatch {
                expected: 2,
                observed: Some(99),
                ..
            })
        ));
    }

    #[test]
    fn base_and_manual_providers_merge_correctly() {
        let summary = catalog_summary(&known_good_base_doc()).expect("valid summary");
        assert_eq!(summary.base_provider_count, 17);
        // Ya no hay providers manuales: `opencode` era un alias de opencode_zen
        // con fila propia y se retiró. La maquinaria de merge sigue viva y la
        // cubre `manual_provider_ids_are_all_present_in_catalog`.
        assert_eq!(summary.manual_provider_count, 0);
        assert_eq!(summary.provider_count, 17);
    }

    #[test]
    fn effective_provider_count_is_17() {
        assert_eq!(
            catalog_summary(&known_good_base_doc())
                .expect("valid summary")
                .provider_count,
            17
        );
    }

    #[test]
    fn catalog_provider_count_is_17() {
        assert_eq!(
            catalog_summary(&known_good_base_doc())
                .expect("valid summary")
                .base_provider_count,
            17
        );
    }

    #[test]
    /// El contrato declara qué providers son manuales y el binario EXIGE que
    /// cada uno exista en el catálogo. Con la lista vacía la exigencia es
    /// trivial, pero el test se conserva: es el que rompe si alguien vuelve a
    /// declarar un manual sin agregar su fila, que fue exactamente el defecto.
    #[test]
    fn manual_provider_ids_are_all_present_in_catalog() {
        let doc = known_good_base_doc();
        let contract = catalog_contract().expect("contrato válido");
        for manual_id in &contract.manual_provider_ids {
            assert!(
                doc.provider(manual_id).is_some(),
                "{manual_id} está declarado manual pero no tiene fila en el catálogo"
            );
        }
        assert!(doc.provider("opencode").is_none(), "la fila alias fue retirada");
    }

    #[test]
    fn provider_ids_are_unique_after_merge() {
        let doc = known_good_base_doc();
        let ids: HashSet<&str> = doc
            .providers
            .iter()
            .map(CatalogProviderEntry::stable_id)
            .collect();
        assert_eq!(ids.len(), doc.providers.len());
        assert!(doc.validate().is_ok());
    }

    #[test]
    fn model_ids_are_valid_after_merge() {
        let doc = known_good_base_doc();
        for provider in &doc.providers {
            assert!(provider
                .user_facing_models()
                .iter()
                .all(|model| !model.trim().is_empty()));
        }
        assert!(doc.validate().is_ok());
    }

    #[test]
    fn footer_selection_exists_in_effective_catalog() {
        let doc = known_good_base_doc();
        let provider = doc.provider("codex_cli").expect("Codex provider");
        assert_eq!(provider.name(), "Codex / OpenAI");
        assert!(provider
            .user_facing_models()
            .contains(&"gpt-5.6-terra".to_string()));
    }

    #[test]
    fn doctor_catalog_checks_pass_for_valid_catalog() {
        let summary = catalog_summary(&known_good_base_doc()).expect("Doctor shared validator");
        assert_eq!(summary.schema_version, Some(2));
        assert!(summary.provider_count > 1);
        assert!(summary.model_count > 1);
    }

    #[test]
    fn catalog_error_identifies_exact_invalid_field() {
        let mut doc = known_good_base_doc();
        let index = doc
            .providers
            .iter()
            .position(|provider| provider.stable_id() == "opencode_zen")
            .expect("OpenCode Free");
        doc.providers[index].models.clear();
        doc.providers[index].models_detected.clear();
        let error = doc
            .validate()
            .expect_err("empty required model list rejected");
        assert!(matches!(
            &error,
            CatalogLoadError::ProviderEntryInvalid {
                provider_id,
                field,
                ..
            } if provider_id == "opencode_zen"
                && field == &format!("providers[{index}].models")
        ));
        assert!(error.to_string().contains("PROVIDER_ENTRY_INVALID"));
    }

    #[test]
    fn tui_and_doctor_share_catalog_validator() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(nexum_acp::provider::catalog_path::INSTALLED_BASE_FILE_NAME);
        std::fs::write(
            &path,
            include_str!("../../../config/provider-catalog-base.json"),
        )
        .unwrap();
        let (doc, loaded_path) = load_catalog_document_from_path(&path).expect("TUI shared loader");
        assert_eq!(loaded_path, path);
        let summary = catalog_summary(&doc).expect("Doctor shared validator");
        assert_eq!(summary.provider_count, 17);
    }

    fn catalog_login_entry(pid: &str, name: &str) -> CatalogLoginEntry {
        CatalogLoginEntry {
            provider_id: pid.to_string(),
            display_name: name.to_string(),
            base_url: format!("https://api.{pid}.example"),
            protocol: "openai".to_string(),
            key_env_hint: format!("consola de {name}"),
            needs_base_url: false,
            static_models: vec![],
        }
    }

    fn sample_doc() -> CatalogDoc {
        CatalogDoc {
            schema_version: Some(2),
            catalog_version: "2".to_string(),
            version: Some("2".to_string()),
            generated_at: "2026-07-04T00:00:00+00:00".to_string(),
            recommended_provider_id: Some("ollama".to_string()),
            active_provider_id: Some("ollama-local".to_string()),
            cli_proxy_api: None,
            providers: vec![
                CatalogProviderEntry {
                    id: "ollama".to_string(),
                    display_name: "Ollama Local".to_string(),
                    status: "usable_now".to_string(),
                    status_detail: "Local Ollama running; model qwen2.5:0.5b certified."
                        .to_string(),
                    recommended: true,
                    usable_now: Some(true),
                    model_policy: CatalogModelPolicy {
                        user_facing_models: vec![
                            "qwen2.5:0.5b".to_string(),
                            "qwen2.5:1.5b".to_string(),
                            "qwen3:1.7b".to_string(),
                        ],
                        user_facing_count: Some(3),
                        reserved_internal_count: Some(1),
                        all_models_reserved: Some(false),
                        selectable_models: vec![
                            "qwen2.5:0.5b".to_string(),
                            "qwen2.5:1.5b".to_string(),
                            "qwen3:1.7b".to_string(),
                        ],
                    },
                    ..Default::default()
                },
                CatalogProviderEntry {
                    id: "opencode-zen".to_string(),
                    display_name: "OpenCode Zen".to_string(),
                    status: "connected".to_string(),
                    recommended: true,
                    credential_detected: true,
                    base_url_detected: Some("https://opencode.ai/zen/go/v1".to_string()),
                    ..Default::default()
                },
                CatalogProviderEntry {
                    id: "codex-cli".to_string(),
                    display_name: "Codex CLI".to_string(),
                    status: "detected_login".to_string(),
                    native_login_detected: Some(true),
                    ..Default::default()
                },
                // Not detected + not in the catalog array → hidden entirely.
                CatalogProviderEntry {
                    id: "qwen".to_string(),
                    display_name: "Qwen".to_string(),
                    status: "requires_api_key".to_string(),
                    ..Default::default()
                },
                CatalogProviderEntry {
                    id: "bridge-simulated".to_string(),
                    display_name: "Bridge Simulated".to_string(),
                    status: "probe_failed".to_string(),
                    bridge_status: Some("simulated".to_string()),
                    models: vec!["simulated-model".to_string()],
                    ..Default::default()
                },
                // Detected (stored key) — its catalog twin must be deduped out.
                CatalogProviderEntry {
                    id: "deepseek".to_string(),
                    provider_id: Some("deepseek".to_string()),
                    display_name: "DeepSeek".to_string(),
                    status: "usable".to_string(),
                    usable_now: Some(true),
                    credential_detected: true,
                    models: vec!["deepseek-v4-pro".to_string()],
                    ..Default::default()
                },
            ],
            catalog: vec![
                catalog_login_entry("anthropic_api_key", "Anthropic / Claude"),
                catalog_login_entry("deepseek", "DeepSeek"),
                CatalogLoginEntry {
                    needs_base_url: true,
                    ..catalog_login_entry("mimo_token_plan", "Xiaomi MiMo Token Plan")
                },
            ],
            notes: Vec::new(),
        }
    }

    #[test]
    fn rebuild_rows_keeps_honest_provider_state_sections() {
        let panel = ProviderPanel::from_catalog(sample_doc());
        let tiers: Vec<u8> = panel
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Header { tier } => Some(*tier),
                _ => None,
            })
            .collect();
        assert_eq!(tiers, vec![0, 2, 3, 4, 5]);
        assert_eq!(tier_title(0), "Disponibles ahora");
        assert_eq!(tier_title(1), "Credencial presente · no utilizable");
        assert_eq!(tier_title(2), "Detectados pero incompletos");
        assert_eq!(tier_title(3), "Soportados no configurados");
        assert_eq!(tier_title(4), "No saludables");
        assert_eq!(tier_title(5), "Catálogo");
    }

    #[test]
    fn supported_providers_remain_visible_without_false_usable_state() {
        let panel = ProviderPanel::from_catalog(sample_doc());
        let ids: Vec<&str> = panel
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Provider { entry, .. } => Some(entry.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            ids,
            vec![
                "ollama",
                "deepseek",
                "opencode-zen",
                "codex-cli",
                "qwen",
                "bridge-simulated",
            ]
        );
        let tiers: Vec<(&str, u8)> = panel
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Provider { tier, entry } => Some((entry.id.as_str(), *tier)),
                _ => None,
            })
            .collect();
        assert!(tiers.contains(&("ollama", 0)));
        assert!(tiers.contains(&("deepseek", 0)));
        assert!(tiers.contains(&("opencode-zen", 2)));
        assert!(tiers.contains(&("codex-cli", 2)));
        assert!(tiers.contains(&("qwen", 3)));
        assert!(tiers.contains(&("bridge-simulated", 4)));
    }

    #[test]
    fn all_supported_not_configured_providers_remain_visible() {
        let providers = (0..18)
            .map(|index| CatalogProviderEntry {
                id: format!("supported-{index}"),
                display_name: format!("Supported {index}"),
                status: "not_configured".to_string(),
                usable_now: Some(false),
                ..Default::default()
            })
            .collect();
        let panel = ProviderPanel::from_catalog(CatalogDoc {
            providers,
            ..Default::default()
        });
        let visible = panel
            .rows
            .iter()
            .filter(|row| matches!(row, Row::Provider { .. }))
            .count();
        assert_eq!(visible, 18);
        assert!(panel
            .rows
            .iter()
            .all(|row| { !matches!(row, Row::Provider { tier, .. } if *tier != 3) }));
    }

    #[test]
    fn catalog_dedupes_detected_providers() {
        let panel = ProviderPanel::from_catalog(sample_doc());
        let catalog_ids: Vec<&str> = panel
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::CatalogEntry { entry } => Some(entry.provider_id.as_str()),
                _ => None,
            })
            .collect();
        // deepseek is already in "Tus proveedores" → NOT in Catálogo.
        assert!(!catalog_ids.contains(&"deepseek"));
        assert!(catalog_ids.contains(&"anthropic_api_key"));
        assert!(catalog_ids.contains(&"mimo_token_plan"));
    }

    #[test]
    fn initial_selection_is_first_selectable_provider() {
        let panel = ProviderPanel::from_catalog(sample_doc());
        assert!(panel.rows[panel.selected].is_selectable());
        assert_eq!(
            panel.selected_entry().map(|e| e.id.as_str()),
            Some("ollama")
        );
    }

    #[test]
    fn move_selection_advances_and_skips_headers() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        assert_eq!(panel.selected_entry().unwrap().id, "ollama");
        panel.move_selection(1);
        assert_eq!(panel.selected_entry().unwrap().id, "deepseek");
        panel.move_selection(5);
        // Crossed into the Catálogo section (header skipped).
        assert_eq!(
            panel.selected_catalog_entry().unwrap().provider_id,
            "anthropic_api_key"
        );
    }

    #[test]
    fn move_selection_clamps_at_ends() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        panel.move_selection(-5); // before first
        assert_eq!(panel.selected_entry().unwrap().id, "ollama");
        panel.move_selection(100); // past last
        assert_eq!(
            panel.selected_catalog_entry().unwrap().provider_id,
            "mimo_token_plan"
        );
    }

    #[test]
    fn tab_moves_to_next_section() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        assert_eq!(panel.rows[panel.selected].tier(), 0);
        panel.move_section(1); // Tab → detectados incompletos (ahora tier 2)
        assert_eq!(panel.rows[panel.selected].tier(), 2);
        assert_eq!(panel.selected_entry().unwrap().id, "opencode-zen");
        panel.move_section(-1); // Shift+Tab vuelve a disponibles
        assert_eq!(panel.rows[panel.selected].tier(), 0);
    }

    #[test]
    fn enter_toggles_expanded_detail_on_provider_rows() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        assert!(panel.expanded_id.is_none());
        panel.toggle_expand();
        assert_eq!(panel.expanded_id.as_deref(), Some("ollama"));
        panel.toggle_expand();
        assert!(panel.expanded_id.is_none());
    }

    #[test]
    fn error_panel_has_no_selectable_rows() {
        let panel = ProviderPanel::from_error("missing");
        assert!(panel.catalog.is_none());
        assert!(panel.error.is_some());
        assert!(panel.rows.is_empty());
        assert_eq!(panel.selectable_count(), 0);
    }

    #[test]
    fn reserved_model_never_in_selectable() {
        let doc = sample_doc();
        let ollama = doc.providers.iter().find(|p| p.id == "ollama").unwrap();
        assert!(!ollama
            .model_policy
            .selectable_models
            .contains(&"qwen3:0.6b".to_string()));
        assert!(!ollama
            .model_policy
            .user_facing_models
            .contains(&"qwen3:0.6b".to_string()));
    }

    // ── One-step login input state machine ──────────────────────────────────

    fn panel_with_input_open(pid: &str) -> ProviderPanel {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        // Move to the Catálogo row for `pid`.
        while panel
            .selected_catalog_entry()
            .map(|e| e.provider_id != pid)
            .unwrap_or(true)
        {
            let before = panel.selected;
            panel.move_selection(1);
            assert_ne!(panel.selected, before, "catalog row {pid} not found");
        }
        assert!(panel.open_input_for_selected());
        panel
    }

    #[test]
    fn enter_on_catalog_row_opens_input() {
        let panel = panel_with_input_open("anthropic_api_key");
        let input = panel.input.as_ref().unwrap();
        assert_eq!(input.entry.provider_id, "anthropic_api_key");
        assert_eq!(input.phase, InputPhase::Editing);
        assert_eq!(input.focus, InputFocus::Key);
    }

    #[test]
    fn zai_glm_visible_en_catalogo_y_enter_abre_modal() {
        // Spec autologin (Parte 10 A/B): Z.ai / GLM Coding Plan aparece en
        // el Catálogo aunque no haya key, y Enter abre el modal de API key.
        let mut doc = sample_doc();
        doc.catalog.push(catalog_login_entry(
            "glm_coding_plan",
            "Z.ai / GLM Coding Plan",
        ));
        let mut panel = ProviderPanel::from_catalog(doc);
        while panel
            .selected_catalog_entry()
            .map(|e| e.provider_id != "glm_coding_plan")
            .unwrap_or(true)
        {
            let before = panel.selected;
            panel.move_selection(1);
            assert_ne!(panel.selected, before, "fila Z.ai/GLM no encontrada");
        }
        assert_eq!(
            panel.selected_catalog_entry().unwrap().display_name,
            "Z.ai / GLM Coding Plan"
        );
        assert!(panel.open_input_for_selected());
        let input = panel.input.as_ref().unwrap();
        assert_eq!(input.entry.provider_id, "glm_coding_plan");
        assert_eq!(input.phase, InputPhase::Editing);
        assert_eq!(input.focus, InputFocus::Key);
    }

    #[test]
    fn open_input_on_provider_row_is_noop() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        assert_eq!(panel.selected_entry().unwrap().id, "ollama");
        assert!(!panel.open_input_for_selected());
        assert!(panel.input.is_none());
    }

    #[test]
    fn typing_and_backspace_edit_key_buffer() {
        let mut panel = panel_with_input_open("anthropic_api_key");
        for c in "sk-abc".chars() {
            panel.input_push_char(c);
        }
        panel.input_backspace();
        assert_eq!(panel.input.as_ref().unwrap().key_buf, "sk-ab");
    }

    #[test]
    fn paste_goes_to_focused_field_first_line_only() {
        let mut panel = panel_with_input_open("anthropic_api_key");
        panel.input_push_str("  sk-pasted-key\nSEGUNDA LINEA IGNORADA");
        assert_eq!(panel.input.as_ref().unwrap().key_buf, "sk-pasted-key");
    }

    #[test]
    fn esc_closes_input_without_persisting() {
        let mut panel = panel_with_input_open("anthropic_api_key");
        panel.input_push_str("sk-whatever");
        panel.close_input();
        assert!(panel.input.is_none());
        assert!(panel.pending_key.is_none());
    }

    #[test]
    fn begin_validation_requires_key() {
        let mut panel = panel_with_input_open("anthropic_api_key");
        assert!(panel.begin_validation().is_none());
        let input = panel.input.as_ref().unwrap();
        assert_eq!(input.phase, InputPhase::Editing);
        assert!(input.error.is_some());
    }

    #[test]
    fn begin_validation_moves_to_validating() {
        let mut panel = panel_with_input_open("anthropic_api_key");
        panel.input_push_str("sk-test-123");
        let req = panel.begin_validation().expect("request");
        assert_eq!(req.0, "anthropic_api_key");
        assert_eq!(req.1, "sk-test-123");
        assert_eq!(panel.input.as_ref().unwrap().phase, InputPhase::Validating);
        assert_eq!(panel.pending_key.as_deref(), Some("sk-test-123"));
    }

    #[test]
    fn mimo_token_plan_requires_base_url_and_dual_focus() {
        let mut panel = panel_with_input_open("mimo_token_plan");
        // Dual-field: focus starts on the base URL.
        assert_eq!(panel.input.as_ref().unwrap().focus, InputFocus::BaseUrl);
        panel.input_toggle_focus();
        assert_eq!(panel.input.as_ref().unwrap().focus, InputFocus::Key);
        panel.input_push_str("sk-mimo-123");
        // Without a base URL, validation refuses and re-focuses the URL field.
        assert!(panel.begin_validation().is_none());
        let input = panel.input.as_ref().unwrap();
        assert_eq!(input.focus, InputFocus::BaseUrl);
        assert!(input.error.is_some());
        // With a base URL it proceeds.
        panel.input_push_str("https://cn.api.mimo.example/v1");
        let req = panel.begin_validation().expect("request");
        assert_eq!(req.2, "https://cn.api.mimo.example/v1");
    }

    #[test]
    fn invalid_key_outcome_clears_key_and_shows_error() {
        let mut panel = panel_with_input_open("anthropic_api_key");
        panel.input_push_str("sk-invalid-test-12345");
        panel.begin_validation().unwrap();
        let success = panel.apply_outcome(LoginOutcome::InvalidKey {
            message: "Key inválida o sin permisos".to_string(),
        });
        assert!(success.is_none());
        assert!(panel.pending_key.is_none(), "nada queda guardado");
        let input = panel.input.as_ref().unwrap();
        assert_eq!(input.phase, InputPhase::Editing);
        assert!(input.key_buf.is_empty(), "el valor previo se borra");
        assert!(input.error.as_deref().unwrap().contains("inválida"));
    }

    #[test]
    fn network_outcome_offers_retry() {
        let mut panel = panel_with_input_open("anthropic_api_key");
        panel.input_push_str("sk-test-123");
        panel.begin_validation().unwrap();
        let success = panel.apply_outcome(LoginOutcome::Network {
            message: "timeout".to_string(),
        });
        assert!(success.is_none());
        assert!(matches!(
            panel.input.as_ref().unwrap().phase,
            InputPhase::NetworkError(_)
        ));
        // The typed key survives so 'r' can retry.
        assert_eq!(panel.input.as_ref().unwrap().key_buf, "sk-test-123");
    }

    // ── Bridge jobs (Sprint C) ──────────────────────────────────────────────

    #[test]
    fn is_bridge_provider_solo_claude_codex_gemini() {
        let mk = |id: &str| CatalogProviderEntry {
            id: id.to_string(),
            provider_id: Some(id.to_string()),
            ..Default::default()
        };
        assert!(ProviderPanel::is_bridge_provider(&mk("claude_code")));
        assert!(ProviderPanel::is_bridge_provider(&mk("codex_cli")));
        assert!(ProviderPanel::is_bridge_provider(&mk("gemini_cli")));
        assert!(!ProviderPanel::is_bridge_provider(&mk("ollama_local")));
        assert!(!ProviderPanel::is_bridge_provider(&mk("opencode_zen")));
        assert!(!ProviderPanel::is_bridge_provider(&mk("mimo_code")));
    }

    // ── El panel no miente por omisión ──────────────────────────────────────

    fn _con_estado(id: &str, estado: &str) -> CatalogProviderEntry {
        CatalogProviderEntry {
            id: id.to_string(),
            provider_id: Some(id.to_string()),
            display_name: id.to_string(),
            status: "detected_login".to_string(),
            usable_now: Some(false),
            credential_detected: true,
            native_login_detected: Some(true),
            connect_kind: Some("credential_store".to_string()),
            credential_state: Some(estado.to_string()),
            credential_detail: Some("credencial válida, cuenta sin saldo".to_string()),
            credential_store: Some("~/.local/share/opencode/auth.json".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn credencial_sin_saldo_sale_de_detectados_incompletos() {
        // No le falta login: le falta crédito. Mezclarlos era la mentira.
        let e = _con_estado("opencode_zen", "verified_no_credit");
        assert_eq!(ProviderPanel::tier_of(&e), 1);
        assert_ne!(ProviderPanel::tier_of(&e), 2, "no es 'detectado incompleto'");
        assert_eq!(tier_title(1), "Credencial presente · no utilizable");
    }

    #[test]
    fn credencial_sin_verificar_o_invalida_van_a_la_misma_seccion() {
        for estado in ["present_unverified", "invalid"] {
            assert_eq!(ProviderPanel::tier_of(&_con_estado("x", estado)), 1);
        }
    }

    #[test]
    fn provider_verificado_y_usable_sigue_en_disponibles() {
        let mut e = _con_estado("mimo_code", "verified");
        e.usable_now = Some(true);
        assert_eq!(ProviderPanel::tier_of(&e), 0);
    }

    #[test]
    fn sin_estado_de_credencial_la_clasificacion_no_cambia() {
        let detectado = CatalogProviderEntry {
            id: "claude_code".to_string(),
            status: "detected_login".to_string(),
            native_login_detected: Some(true),
            ..Default::default()
        };
        assert_eq!(ProviderPanel::tier_of(&detectado), 2);
    }

    #[test]
    fn el_tier_del_catalogo_es_consistente_entre_fila_y_header() {
        // Cuando divergían, move_section saltaba a una sección fantasma.
        let fila = Row::CatalogEntry {
            entry: CatalogLoginEntry::default(),
        };
        assert_eq!(fila.tier(), TIER_CATALOG);
        assert_eq!(tier_title(TIER_CATALOG), "Catálogo");
    }

    #[test]
    fn enter_sin_flujo_siempre_da_un_motivo() {
        for kind in ["api_key_input", "bridge_oauth", "otro"] {
            let e = CatalogProviderEntry {
                id: "x".to_string(),
                display_name: "X".to_string(),
                connect_kind: Some(kind.to_string()),
                ..Default::default()
            };
            let motivo = ProviderPanel::no_flow_reason(&e);
            assert!(!motivo.is_empty());
            assert!(motivo.contains("Remedio"), "kind {kind} sin remedio");
        }
    }

    // ── RC-7: la acción de connect se deriva del catálogo, no de ids fijos ───

    #[test]
    fn connect_kind_del_catalogo_manda_sobre_la_lista_fija() {
        let mut e = CatalogProviderEntry {
            id: "provider_nuevo_que_no_existia".to_string(),
            provider_id: Some("provider_nuevo_que_no_existia".to_string()),
            ..Default::default()
        };
        // Un provider que NO está en BRIDGE_PROVIDERS igual puede ser de puente
        // si el catálogo lo declara: agregar uno ya no requiere tocar código.
        e.connect_kind = Some("bridge_oauth".to_string());
        assert!(ProviderPanel::is_bridge_provider(&e));
    }

    #[test]
    fn connect_kind_credential_store_no_es_de_puente() {
        let e = CatalogProviderEntry {
            id: "opencode_zen".to_string(),
            provider_id: Some("opencode_zen".to_string()),
            connect_kind: Some("credential_store".to_string()),
            ..Default::default()
        };
        assert!(!ProviderPanel::is_bridge_provider(&e));
        assert!(ProviderPanel::is_credential_store_provider(&e));
    }

    #[test]
    fn sin_connect_kind_cae_al_auth_mode_y_luego_a_la_lista_fija() {
        // Compatibilidad con catálogos viejos que no publican el campo.
        let por_auth_mode = CatalogProviderEntry {
            id: "cualquiera".to_string(),
            auth_mode: Some("cli_oauth".to_string()),
            ..Default::default()
        };
        assert!(ProviderPanel::is_bridge_provider(&por_auth_mode));

        let por_lista = CatalogProviderEntry {
            id: "claude_code".to_string(),
            provider_id: Some("claude_code".to_string()),
            ..Default::default()
        };
        assert!(ProviderPanel::is_bridge_provider(&por_lista));
    }

    #[test]
    fn hint_de_almacen_dice_de_donde_salio_y_que_hacer() {
        let e = CatalogProviderEntry {
            id: "opencode_zen".to_string(),
            display_name: "OpenCode Zen".to_string(),
            connect_kind: Some("credential_store".to_string()),
            credential_store: Some("~/.local/share/opencode/auth.json".to_string()),
            credential_detail: Some("credencial válida, cuenta sin saldo".to_string()),
            relogin_command: Some("opencode auth login".to_string()),
            ..Default::default()
        };
        let hint = ProviderPanel::credential_store_hint(&e);
        assert!(hint.contains("auth.json"), "debe decir de qué almacén salió");
        assert!(hint.contains("sin saldo"), "debe decir en qué estado quedó");
        assert!(hint.contains("opencode auth login"), "debe dar el remedio");
    }

    #[test]
    fn hint_de_almacen_sin_datos_igual_da_un_remedio() {
        let e = CatalogProviderEntry {
            id: "x".to_string(),
            display_name: "X".to_string(),
            connect_kind: Some("credential_store".to_string()),
            ..Default::default()
        };
        let hint = ProviderPanel::credential_store_hint(&e);
        assert!(hint.contains("Remedio"), "nunca queda en silencio");
    }

    // ── RC-8: el error de una acción se ve DENTRO del panel ──────────────────

    #[test]
    fn action_error_arranca_vacio() {
        let panel = ProviderPanel::from_catalog(sample_doc());
        assert!(panel.action_error.is_none());
    }

    #[test]
    fn set_action_error_no_destruye_el_catalogo() {
        // A diferencia de `error`, que es terminal, el banner convive con las
        // filas: el usuario ve la causa sin perder el panel.
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        let rows_antes = panel.rows.len();
        panel.set_action_error("Claude Code — no se pudo conectar");
        assert_eq!(panel.action_error.as_deref(), Some("Claude Code — no se pudo conectar"));
        assert!(panel.catalog.is_some(), "el catálogo sobrevive al error");
        assert!(panel.error.is_none(), "no es un error terminal");
        assert_eq!(panel.rows.len(), rows_antes, "las filas siguen ahí");
    }

    #[test]
    fn clear_action_error_limpia_el_banner() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        panel.set_action_error("falló");
        panel.clear_action_error();
        assert!(panel.action_error.is_none());
    }

    #[test]
    fn start_refresh_limpia_el_error_anterior() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        panel.set_action_error("error viejo");
        panel.start_refresh();
        assert!(
            panel.action_error.is_none(),
            "una acción nueva empieza sin el error de la anterior"
        );
        panel.cancel_bridge_job();
    }

    #[test]
    fn banner_de_error_agranda_el_panel() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        let alto_antes = panel.desired_height(60, 120);
        panel.set_action_error("linea 1\nlinea 2\nlinea 3");
        assert!(
            panel.desired_height(60, 120) > alto_antes,
            "el banner necesita lugar propio o taparía filas"
        );
    }

    #[test]
    fn bridge_job_cancel_limpia_estado() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        panel.bridge_job = Some(BridgeJob {
            label: "x".to_string(),
            started: Instant::now(),
            timeout_secs: 60,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        panel.cancel_bridge_job();
        assert!(panel.bridge_job.is_none());
        assert!(panel.poll_bridge_job().is_none(), "sin job no hay outcome");
    }

    #[test]
    fn bridge_job_timeout_devuelve_error() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        let (_tx, rx) = mpsc::channel::<BridgeJobOutcome>();
        panel.bridge_job_rx = Some(Arc::new(Mutex::new(rx)));
        panel.bridge_job = Some(BridgeJob {
            label: "x".to_string(),
            started: Instant::now() - std::time::Duration::from_secs(999),
            timeout_secs: 60,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        match panel.poll_bridge_job() {
            Some(BridgeJobOutcome::RefreshDone(Err(msg))) => {
                assert!(msg.contains("Timeout"))
            }
            other => panic!("se esperaba timeout, hubo {other:?}"),
        }
        assert!(panel.bridge_job.is_none());
    }

    #[test]
    fn bridge_job_recibe_outcome_del_canal() {
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        let (tx, rx) = mpsc::channel::<BridgeJobOutcome>();
        panel.bridge_job_rx = Some(Arc::new(Mutex::new(rx)));
        panel.bridge_job = Some(BridgeJob {
            label: "x".to_string(),
            started: Instant::now(),
            timeout_secs: 60,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        tx.send(BridgeJobOutcome::ConnectDone {
            family: "Codex / OpenAI".to_string(),
            result: Ok(()),
            callback_diag: CallbackDiag::default(),
        })
        .unwrap();
        match panel.poll_bridge_job() {
            Some(BridgeJobOutcome::ConnectDone { family, result, .. }) => {
                assert_eq!(family, "Codex / OpenAI");
                assert!(result.is_ok());
            }
            other => panic!("outcome inesperado: {other:?}"),
        }
        assert!(panel.bridge_job.is_none(), "el job se limpia al consumir");
    }

    #[test]
    fn cancel_token_setea_flag_y_nuevo_connect_cancela_anterior() {
        // Esc cancela: el flag del job viejo se setea a true.
        let mut panel = ProviderPanel::from_catalog(sample_doc());
        let job_cancel = Arc::new(AtomicBool::new(false));
        panel.bridge_job = Some(BridgeJob {
            label: "viejo".to_string(),
            started: Instant::now(),
            timeout_secs: 60,
            cancel: job_cancel.clone(),
        });
        panel.cancel_bridge_job();
        assert!(
            job_cancel.load(Ordering::SeqCst),
            "Esc debe setear el flag cancel"
        );
        assert!(panel.bridge_job.is_none());
    }

    #[test]
    fn success_outcome_hands_over_key_once_and_closes_input() {
        let mut panel = panel_with_input_open("anthropic_api_key");
        panel.input_push_str("sk-valid-123");
        panel.begin_validation().unwrap();
        let success = panel
            .apply_outcome(LoginOutcome::Success {
                provider_id: "anthropic_api_key".to_string(),
                display_name: "Anthropic / Claude".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                protocol: "anthropic".to_string(),
                models: vec!["claude-opus-4-8".to_string()],
            })
            .expect("success data");
        assert_eq!(success.api_key, "sk-valid-123");
        assert_eq!(success.protocol, "anthropic");
        assert!(panel.input.is_none());
        assert!(
            panel.pending_key.is_none(),
            "la key se entrega una sola vez"
        );
    }

    #[test]
    fn catalog_resolution_rejects_corrupt_live_and_uses_previous() {
        let tmp = tempfile::tempdir().unwrap();
        let live = tmp.path().join(nexum_acp::provider::catalog_path::LIVE_CATALOG_FILE_NAME);
        let previous = tmp.path().join(nexum_acp::provider::catalog_path::PREVIOUS_CATALOG_FILE_NAME);
        let base = tmp.path().join(nexum_acp::provider::catalog_path::INSTALLED_BASE_FILE_NAME);
        std::fs::write(&live, "{").unwrap();
        std::fs::write(&previous, r#"{"providers":[]}"#).unwrap();
        std::fs::write(&base, r#"{"providers":[]}"#).unwrap();

        let resolution = resolve_catalog_from_candidates(&live, &previous, &base);
        assert_eq!(resolution.source, CatalogSource::Previous);
        assert_eq!(resolution.path, previous);
        assert!(resolution.live_rejected);
    }

    #[test]
    fn catalog_resolution_uses_installed_base_when_live_and_previous_are_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let live = tmp.path().join(nexum_acp::provider::catalog_path::LIVE_CATALOG_FILE_NAME);
        let previous = tmp.path().join(nexum_acp::provider::catalog_path::PREVIOUS_CATALOG_FILE_NAME);
        let base = tmp.path().join(nexum_acp::provider::catalog_path::INSTALLED_BASE_FILE_NAME);
        std::fs::write(&base, r#"{"providers":[]}"#).unwrap();

        let resolution = resolve_catalog_from_candidates(&live, &previous, &base);

        assert_eq!(resolution.source, CatalogSource::Base);
        assert_eq!(resolution.path, base);
        assert!(!resolution.live_rejected);
    }
}
