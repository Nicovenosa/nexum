use crate::{app::App, command::Command};

/// `/proveedor` and `/provider` open the read-only Provider Catalog panel.
///
/// This is a dedicated panel (NOT an alias of /login). It renders
/// el catálogo resuelto por `catalog_path` and never shows inherited/fake data
/// (no Opus/Sonnet/Haiku, no "(openai)" subtitle, no reserved qwen3:0.6b as a
/// user-facing model). /login continues to exist separately for provider CRUD.
pub struct ProviderCommand;

impl Command for ProviderCommand {
    fn name(&self) -> &str {
        "provider"
    }

    fn aliases(&self) -> Vec<&str> {
        // "provedor" (sin la segunda e) es el alias que usa Nico a diario.
        vec!["proveedor", "provedor"]
    }

    fn description(&self, _lc: &crate::i18n::LcRegistry) -> String {
        "Abrir el catálogo de proveedores".to_string()
    }

    fn execute(&self, app: &mut App, _args: &str) {
        app.open_provider_panel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_command_name_and_aliases() {
        let cmd = ProviderCommand;
        assert_eq!(cmd.name(), "provider");
        assert!(cmd.aliases().contains(&"proveedor"));
        assert!(!cmd.aliases().contains(&"login"));
    }

    #[test]
    fn login_command_no_longer_owns_proveedor_alias() {
        // /proveedor and /provider must be owned exclusively by ProviderCommand
        // so dispatching them opens the catalog panel, not the login editor.
        let login = super::super::login::LoginCommand;
        assert!(login.aliases().is_empty());
        assert_ne!(login.name(), "provider");
    }
}
