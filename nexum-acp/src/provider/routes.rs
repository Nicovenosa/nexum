//! Typed execution routes shared by the TUI, Doctor and ACP runtime.
//!
//! The provider catalog describes what is visible. This registry describes
//! how a `(provider_id, catalog_model_id)` pair is executed. A route is always
//! selected by both identifiers; resolving a model against the previously
//! active provider is rejected instead of silently crossing providers.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub const PROVIDER_ROUTE_SCHEMA_VERSION: u32 = 1;
pub const PROVIDER_ROUTE_REGISTRY_FILE: &str = "provider-route-registry.json";
pub const INSTALLED_PROVIDER_RESOLVER: &str =
    "libexec/nexum/providers/nexum_providers/provider_resolve.py";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderRouteRegistry {
    pub schema_version: u32,
    pub routes: Vec<ProviderExecutionRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderExecutionRoute {
    pub provider_id: String,
    pub auth_mode: String,
    pub adapter: String,
    pub resolver: String,
    pub endpoint_or_cli: String,
    pub upstream_provider: String,
    pub model_mapping: String,
    #[serde(default)]
    pub model_overrides: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedProviderRoute {
    pub provider_id: String,
    pub catalog_model_id: String,
    pub adapter: String,
    pub upstream_provider: String,
    pub upstream_model: String,
    pub auth_mode: String,
    pub endpoint_or_cli: String,
    pub resolver: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderRouteError {
    #[error("PROVIDER_ROUTE_REGISTRY_NOT_FOUND: {path}")]
    NotFound { path: PathBuf },
    #[error("PROVIDER_ROUTE_REGISTRY_PARSE_ERROR: {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("PROVIDER_ROUTE_SCHEMA_MISMATCH: expected={expected} observed={observed}")]
    SchemaMismatch { expected: u32, observed: u32 },
    #[error("DUPLICATE_PROVIDER_ROUTE: provider_id={provider_id}")]
    DuplicateProvider { provider_id: String },
    #[error("INVALID_PROVIDER_ROUTE: provider_id={provider_id} field={field}")]
    InvalidRoute {
        provider_id: String,
        field: &'static str,
    },
    #[error("MISSING_PROVIDER_ROUTE: provider_id={provider_id}")]
    MissingProvider { provider_id: String },
    #[error("MODEL_NOT_AVAILABLE: provider_id={provider_id} model_id={model_id}")]
    ModelNotAvailable {
        provider_id: String,
        model_id: String,
    },
    #[error("CATALOG_ROUTE_PROVIDER_MISMATCH: {detail}")]
    CatalogProviderMismatch { detail: String },
    #[error("PROVIDER_ROUTE_CATALOG_NOT_FOUND: {path}")]
    CatalogNotFound { path: PathBuf },
    #[error("PROVIDER_ROUTE_CATALOG_PARSE_ERROR: {path}: {message}")]
    CatalogParse { path: PathBuf, message: String },
}

impl ProviderRouteRegistry {
    pub fn load_from_path(path: &Path) -> Result<Self, ProviderRouteError> {
        let raw = fs::read_to_string(path).map_err(|_| ProviderRouteError::NotFound {
            path: path.to_path_buf(),
        })?;
        let registry: Self =
            serde_json::from_str(&raw).map_err(|error| ProviderRouteError::Parse {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn load_installed() -> Result<(Self, PathBuf), ProviderRouteError> {
        let path = installed_route_registry_path()?;
        let registry = Self::load_from_path(&path)?;
        Ok((registry, path))
    }

    pub fn validate(&self) -> Result<(), ProviderRouteError> {
        if self.schema_version != PROVIDER_ROUTE_SCHEMA_VERSION {
            return Err(ProviderRouteError::SchemaMismatch {
                expected: PROVIDER_ROUTE_SCHEMA_VERSION,
                observed: self.schema_version,
            });
        }
        let mut ids = BTreeSet::new();
        for route in &self.routes {
            if !ids.insert(route.provider_id.as_str()) {
                return Err(ProviderRouteError::DuplicateProvider {
                    provider_id: route.provider_id.clone(),
                });
            }
            for (field, value) in [
                ("provider_id", route.provider_id.as_str()),
                ("auth_mode", route.auth_mode.as_str()),
                ("adapter", route.adapter.as_str()),
                ("resolver", route.resolver.as_str()),
                ("endpoint_or_cli", route.endpoint_or_cli.as_str()),
                ("upstream_provider", route.upstream_provider.as_str()),
            ] {
                if value.trim().is_empty() {
                    return Err(ProviderRouteError::InvalidRoute {
                        provider_id: route.provider_id.clone(),
                        field,
                    });
                }
            }
            if route.model_mapping != "identity" {
                return Err(ProviderRouteError::InvalidRoute {
                    provider_id: route.provider_id.clone(),
                    field: "model_mapping",
                });
            }
        }
        Ok(())
    }

    pub fn route(&self, provider_id: &str) -> Result<&ProviderExecutionRoute, ProviderRouteError> {
        self.routes
            .iter()
            .find(|route| route.provider_id == provider_id)
            .ok_or_else(|| ProviderRouteError::MissingProvider {
                provider_id: provider_id.to_string(),
            })
    }

    pub fn resolve(
        &self,
        provider_id: &str,
        catalog_model_id: &str,
    ) -> Result<ResolvedProviderRoute, ProviderRouteError> {
        if catalog_model_id.trim().is_empty() {
            return Err(ProviderRouteError::ModelNotAvailable {
                provider_id: provider_id.to_string(),
                model_id: catalog_model_id.to_string(),
            });
        }
        let route = self.route(provider_id)?;
        let upstream_model = route
            .model_overrides
            .get(catalog_model_id)
            .cloned()
            .unwrap_or_else(|| catalog_model_id.to_string());
        Ok(ResolvedProviderRoute {
            provider_id: provider_id.to_string(),
            catalog_model_id: catalog_model_id.to_string(),
            adapter: route.adapter.clone(),
            upstream_provider: route.upstream_provider.clone(),
            upstream_model,
            auth_mode: route.auth_mode.clone(),
            endpoint_or_cli: route.endpoint_or_cli.clone(),
            resolver: route.resolver.clone(),
        })
    }

    pub fn validate_catalog(
        &self,
        catalog: &[(String, Vec<String>)],
    ) -> Result<(), ProviderRouteError> {
        let catalog_ids: BTreeSet<&str> = catalog
            .iter()
            .map(|(provider, _)| provider.as_str())
            .collect();
        let route_ids: BTreeSet<&str> = self
            .routes
            .iter()
            .map(|route| route.provider_id.as_str())
            .collect();
        if catalog_ids != route_ids {
            let missing_routes: Vec<_> = catalog_ids.difference(&route_ids).copied().collect();
            let missing_catalog: Vec<_> = route_ids.difference(&catalog_ids).copied().collect();
            return Err(ProviderRouteError::CatalogProviderMismatch {
                detail: format!(
                    "missing_routes={missing_routes:?} missing_catalog={missing_catalog:?}"
                ),
            });
        }
        for (provider_id, models) in catalog {
            self.route(provider_id)?;
            for model in models {
                self.resolve_catalog_model(catalog, provider_id, model)?;
            }
        }
        Ok(())
    }

    pub fn resolve_catalog_model(
        &self,
        catalog: &[(String, Vec<String>)],
        provider_id: &str,
        model_id: &str,
    ) -> Result<ResolvedProviderRoute, ProviderRouteError> {
        let models = catalog
            .iter()
            .find(|(candidate, _)| candidate == provider_id)
            .ok_or_else(|| ProviderRouteError::MissingProvider {
                provider_id: provider_id.to_string(),
            })?
            .1
            .as_slice();
        if !models.is_empty() && !models.iter().any(|candidate| candidate == model_id) {
            return Err(ProviderRouteError::ModelNotAvailable {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
            });
        }
        self.resolve(provider_id, model_id)
    }
}

pub fn installed_route_registry_path() -> Result<PathBuf, ProviderRouteError> {
    if let Some(path) = std::env::var_os("NEXUM_PROVIDER_ROUTE_REGISTRY") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(ProviderRouteError::NotFound { path });
    }
    let sibling = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .map(|slot| slot.join(PROVIDER_ROUTE_REGISTRY_FILE))
    });
    if let Some(path) = sibling.as_ref().filter(|path| path.is_file()) {
        return Ok(path.clone());
    }
    #[cfg(debug_assertions)]
    {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../config")
            .join(PROVIDER_ROUTE_REGISTRY_FILE);
        if source.is_file() {
            return Ok(source);
        }
    }
    Err(ProviderRouteError::NotFound {
        path: sibling.unwrap_or_else(|| PathBuf::from(PROVIDER_ROUTE_REGISTRY_FILE)),
    })
}

pub fn provider_resolver_for_executable(executable: &Path) -> Result<PathBuf, ProviderRouteError> {
    let slot = executable
        .parent()
        .ok_or_else(|| ProviderRouteError::NotFound {
            path: executable.to_path_buf(),
        })?;
    let installed = slot.join(INSTALLED_PROVIDER_RESOLVER);
    if installed.is_file() {
        return Ok(installed);
    }
    Err(ProviderRouteError::NotFound { path: installed })
}

pub fn installed_provider_resolver_path() -> Result<PathBuf, ProviderRouteError> {
    let executable = std::env::current_exe().map_err(|_| ProviderRouteError::NotFound {
        path: PathBuf::from("current_exe"),
    })?;
    match provider_resolver_for_executable(&executable) {
        Ok(path) => Ok(path),
        Err(installed_error) => {
            #[cfg(debug_assertions)]
            {
                let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../src/nexum_providers/provider_resolve.py");
                if source.is_file() {
                    return Ok(source);
                }
            }
            Err(installed_error)
        }
    }
}

/// Catálogo que gobierna la resolución de rutas de ejecución.
///
/// ANTES leía directo `<slot>/provider-catalog-output.json`, la copia congelada
/// del base que deja el empaquetador. El panel, en cambio, leía el catálogo
/// vivo de `reconcile`. Con dos archivos distintos el panel mostraba un
/// provider como usable y el turno salía al endpoint de otro: eso era el 502 de
/// opencode_zen y ollama_local. Ahora los dos pasan por `catalog_path::resolve`,
/// así que la divergencia deja de ser posible por construcción.
pub fn installed_catalog_path() -> Result<PathBuf, ProviderRouteError> {
    let resolution = super::catalog_path::resolve();
    if resolution.source == super::catalog_path::CatalogSource::Missing {
        return Err(ProviderRouteError::CatalogNotFound {
            path: resolution.path,
        });
    }
    Ok(resolution.path)
}

pub fn catalog_pairs_from_path(
    path: &Path,
) -> Result<Vec<(String, Vec<String>)>, ProviderRouteError> {
    let raw = fs::read_to_string(path).map_err(|_| ProviderRouteError::CatalogNotFound {
        path: path.to_path_buf(),
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|error| ProviderRouteError::CatalogParse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    let providers = value
        .get("providers")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ProviderRouteError::CatalogParse {
            path: path.to_path_buf(),
            message: "providers must be an array".to_string(),
        })?;
    providers
        .iter()
        .map(|provider| {
            let id = provider
                .get("provider_id")
                .or_else(|| provider.get("id"))
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| ProviderRouteError::CatalogParse {
                    path: path.to_path_buf(),
                    message: "provider_id must be a string".to_string(),
                })?
                .to_string();
            let models = provider
                .get("models")
                .or_else(|| provider.get("models_detected"))
                .and_then(serde_json::Value::as_array)
                .map(|models| {
                    models
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Ok((id, models))
        })
        .collect()
}

pub fn validate_installed_registry() -> Result<(ProviderRouteRegistry, PathBuf), ProviderRouteError>
{
    let (registry, path) = ProviderRouteRegistry::load_installed()?;
    let catalog = catalog_pairs_from_path(&installed_catalog_path()?)?;
    registry.validate_catalog(&catalog)?;
    Ok((registry, path))
}

pub fn validate_installed_selection(
    provider_id: &str,
    model_id: &str,
) -> Result<ResolvedProviderRoute, ProviderRouteError> {
    let (registry, _) = validate_installed_registry()?;
    let catalog = catalog_pairs_from_path(&installed_catalog_path()?)?;
    // Providers configured manually may have dynamic models. Catalog providers
    // with enumerated models are strict and never borrow a model from another
    // provider that happens to use the same OpenAI-compatible adapter.
    registry.resolve_catalog_model(&catalog, provider_id, model_id)
}

/// Runtime enforcement wrapper. Synthetic provider IDs are accepted only
/// while compiling nexum-acp's own unit tests; production and dependent
/// crates always use the installed catalog/registry contract.
pub fn enforce_runtime_selection(
    provider_id: &str,
    model_id: &str,
) -> Result<(), ProviderRouteError> {
    match validate_installed_selection(provider_id, model_id) {
        Ok(_) => Ok(()),
        #[cfg(test)]
        Err(ProviderRouteError::MissingProvider { .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn source_registry() -> ProviderRouteRegistry {
        ProviderRouteRegistry::load_from_path(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../config/provider-route-registry.json"),
        )
        .unwrap()
    }

    fn source_catalog() -> Vec<(String, Vec<String>)> {
        let value: Value = serde_json::from_str(
            &fs::read_to_string(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../config/provider-catalog-base.json"),
            )
            .unwrap(),
        )
        .unwrap();
        value["providers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|provider| {
                let id = provider
                    .get("provider_id")
                    .or_else(|| provider.get("id"))
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_string();
                let models = provider["models"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|model| model.as_str().unwrap().to_string())
                    .collect();
                (id, models)
            })
            .collect()
    }

    #[test]
    fn mimo_v25_has_provider_route() {
        source_registry().resolve("mimo_code", "mimo-v2.5").unwrap();
    }

    #[test]
    fn mimo_v25_maps_to_valid_upstream() {
        let route = source_registry().resolve("mimo_code", "mimo-v2.5").unwrap();
        assert_eq!(route.upstream_provider, "mimo");
        assert_eq!(route.upstream_model, "mimo-v2.5");
        assert!(!route.endpoint_or_cli.contains("8317"));
    }

    #[test]
    fn mimo_route_does_not_use_unknown_provider() {
        let route = source_registry().resolve("mimo_code", "mimo-v2.5").unwrap();
        assert_ne!(route.upstream_provider, "unknown");
        assert_ne!(route.upstream_provider, "codex");
    }

    #[test]
    fn provider_model_mapping_is_complete() {
        source_registry()
            .validate_catalog(&source_catalog())
            .unwrap();
    }

    #[test]
    fn every_visible_provider_has_execution_route() {
        let registry = source_registry();
        for (provider, _) in source_catalog() {
            registry.route(&provider).unwrap();
        }
    }

    #[test]
    fn every_visible_model_has_execution_mapping() {
        let registry = source_registry();
        for (provider, models) in source_catalog() {
            for model in models {
                registry.resolve(&provider, &model).unwrap();
            }
        }
    }

    #[test]
    fn catalog_and_route_registry_provider_ids_match() {
        source_registry()
            .validate_catalog(&source_catalog())
            .unwrap();
    }

    #[test]
    fn cli_provider_does_not_require_http_api_key() {
        let registry = source_registry();
        for provider in ["codex_cli", "claude_code", "gemini_cli"] {
            assert_eq!(registry.route(provider).unwrap().auth_mode, "cli_oauth");
        }
        for provider in ["opencode_zen", "opencode_go"] {
            assert_eq!(registry.route(provider).unwrap().auth_mode, "cli_account");
        }
    }

    #[test]
    fn codex_real_route_passes() {
        source_registry().resolve("codex_cli", "gpt-5.4").unwrap();
    }

    #[test]
    fn claude_code_real_route_passes() {
        source_registry()
            .resolve("claude_code", "claude-sonnet-4-6")
            .unwrap();
    }

    #[test]
    fn retired_claude_model_has_no_execution_mapping() {
        let error = source_registry()
            .resolve_catalog_model(
                &source_catalog(),
                "claude_code",
                "claude-sonnet-4-20250514",
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ProviderRouteError::ModelNotAvailable { .. }
        ));
    }

    #[test]
    fn retired_claude_model_request_is_structured_error() {
        let error = source_registry()
            .resolve_catalog_model(
                &source_catalog(),
                "claude_code",
                "claude-sonnet-4-20250514",
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "MODEL_NOT_AVAILABLE: provider_id=claude_code model_id=claude-sonnet-4-20250514"
        );
    }

    #[test]
    fn current_claude_model_has_execution_mapping() {
        let route = source_registry()
            .resolve_catalog_model(&source_catalog(), "claude_code", "claude-sonnet-4-6")
            .unwrap();
        assert_eq!(route.catalog_model_id, "claude-sonnet-4-6");
        assert_eq!(route.upstream_model, "claude-sonnet-4-6");
    }

    #[test]
    fn gemini_cli_real_route_passes() {
        source_registry()
            .resolve("gemini_cli", "gemini-3-flash")
            .unwrap();
    }

    #[test]
    fn mimo_real_route_passes() {
        source_registry().resolve("mimo_code", "mimo-v2.5").unwrap();
    }

    #[test]
    fn opencode_free_real_route_passes() {
        source_registry()
            .resolve("opencode_zen", "deepseek-v4-flash-free")
            .unwrap();
    }

    #[test]
    fn opencode_go_real_route_passes() {
        source_registry()
            .resolve("opencode_go", "deepseek-v4-flash")
            .unwrap();
    }
}
