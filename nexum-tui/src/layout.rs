//! InstalledLayoutV1 is the only source for runtime resource paths.

use std::path::{Path, PathBuf};

use nexum_acp::provider::catalog_path::{INSTALLED_BASE_FILE_NAME, PACKAGED_BASE_FILE_NAME};

const EXECUTABLE: &str = "nexum";
const REQUIRED_FILES: &[&str] = &[
    "nexum",
    "nexum-acp-host",
    "nexum-autologin-reconcile",
    INSTALLED_BASE_FILE_NAME,
    PACKAGED_BASE_FILE_NAME,
    "reserved-models.json",
    "LICENSE",
    "NOTICE",
    "PACKAGE_MANIFEST.json",
];
const REQUIRED_DIRS: &[&str] = &["src/nexum_providers", "schemas"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledLayoutV1 {
    version_root: PathBuf,
}

impl InstalledLayoutV1 {
    /// Resolves an executable path to its canonical installed version root.
    /// Checkout paths are rejected because they do not satisfy the complete layout.
    pub fn from_executable(executable: &Path) -> Option<Self> {
        let executable = executable.canonicalize().ok()?;
        if executable.file_name()? != EXECUTABLE {
            return None;
        }
        Self::from_version_root(executable.parent()?)
    }

    pub fn current() -> Option<Self> {
        #[cfg(test)]
        if let Ok(root) = std::env::var("NEXUM_RESOURCE_ROOT") {
            if !root.is_empty() {
                return Self::from_version_root(Path::new(&root));
            }
        }

        std::env::current_exe()
            .ok()
            .as_deref()
            .and_then(Self::from_executable)
    }

    pub fn from_version_root(version_root: &Path) -> Option<Self> {
        let version_root = version_root.canonicalize().ok()?;
        if REQUIRED_FILES
            .iter()
            .all(|name| resource_is_contained(&version_root, name, true))
            && REQUIRED_DIRS
                .iter()
                .all(|name| resource_is_contained(&version_root, name, false))
        {
            Some(Self { version_root })
        } else {
            None
        }
    }

    pub fn version_root(&self) -> PathBuf {
        self.version_root.clone()
    }

    pub fn executable(&self) -> PathBuf {
        self.version_root.join(EXECUTABLE)
    }

    pub fn acp_host(&self) -> PathBuf {
        self.version_root.join("nexum-acp-host")
    }

    pub fn reconcile(&self) -> PathBuf {
        self.version_root.join("nexum-autologin-reconcile")
    }

    /// Nombre desde `catalog_path`: esta capa PRODUCE el archivo que aquel
    /// RESUELVE, así que repetir el literal era la última copia del grep.
    pub fn catalog_output(&self) -> PathBuf {
        self.version_root.join(INSTALLED_BASE_FILE_NAME)
    }

    pub fn base_catalog(&self) -> PathBuf {
        self.version_root.join(PACKAGED_BASE_FILE_NAME)
    }

    pub fn reserved_models(&self) -> PathBuf {
        self.version_root.join("reserved-models.json")
    }

    pub fn provider_package(&self) -> PathBuf {
        self.version_root.join("src/nexum_providers")
    }

    pub fn schemas(&self) -> PathBuf {
        self.version_root.join("schemas")
    }

    pub fn license(&self) -> PathBuf {
        self.version_root.join("LICENSE")
    }

    pub fn notice(&self) -> PathBuf {
        self.version_root.join("NOTICE")
    }

    pub fn manifest(&self) -> PathBuf {
        self.version_root.join("PACKAGE_MANIFEST.json")
    }
}

fn resource_is_contained(version_root: &Path, name: &str, file: bool) -> bool {
    let Ok(resource) = version_root.join(name).canonicalize() else {
        return false;
    };
    resource.starts_with(version_root) && if file { resource.is_file() } else { resource.is_dir() }
}

#[cfg(test)]
#[path = "layout_test.rs"]
mod tests;

/// Identidad del slot instalado para el footer: `<slot> · <sha8 del binario>`.
///
/// Toda captura de pantalla se autoidentifica. Sin esto, mirar una foto de la
/// TUI no dice qué binario la produjo — que es exactamente lo que costó caro
/// cuando el catálogo y el binario quedaron desfasados.
///
/// Se calcula UNA vez: hashear 20 MB en cada frame sería absurdo.
pub fn slot_identity() -> &'static str {
    use std::sync::OnceLock;
    static IDENTITY: OnceLock<String> = OnceLock::new();
    IDENTITY.get_or_init(|| {
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(_) => return String::new(),
        };
        // El slot es el directorio que contiene al binario, resuelto por el
        // symlink `current` para que muestre el slot REAL, no el alias.
        let slot = exe
            .canonicalize()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        let sha = std::fs::read(&exe)
            .ok()
            .map(|bytes| {
                use sha2::{Digest, Sha256};
                let digest = Sha256::digest(&bytes);
                digest.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>()
            })
            .unwrap_or_default();
        match (slot.is_empty(), sha.is_empty()) {
            (false, false) => format!("{slot} · {sha}"),
            (false, true) => slot,
            _ => String::new(),
        }
    })
}
