//! FUENTE ÚNICA de la ruta del catálogo de providers.
//!
//! Existe porque no existía. Había tres construcciones independientes de la
//! misma ruta —el panel, el catálogo vivo de `nexum-acp`, y el resolvedor de
//! rutas de ejecución— y dos de ellas apuntaban a archivos distintos: el panel
//! leía el catálogo vivo que publica `reconcile` y el resolvedor leía la copia
//! congelada que el empaquetador deja en el slot. El resultado fue un panel que
//! mostraba un provider como usable mientras el turno se iba a otro endpoint y
//! volvía 502. Es el mismo defecto que ya nos costó siete bugs: dos lugares para
//! la misma verdad, y ningún mecanismo que grite cuando se separan.
//!
//! Todo consumidor —producción y fixtures de test— resuelve por acá.

use std::path::{Path, PathBuf};

/// Nombre del catálogo vivo que publica `reconcile`.
pub const LIVE_CATALOG_FILE_NAME: &str = "provider-catalog-live.json";
/// Snapshot del último catálogo vivo que validó.
pub const PREVIOUS_CATALOG_FILE_NAME: &str = "provider-catalog-live.previous.json";
/// Catálogo base que viaja en el slot instalado.
pub const INSTALLED_BASE_FILE_NAME: &str = "provider-catalog-output.json";
/// Catálogo base tal como lo nombra el empaquetador dentro del slot.
pub const PACKAGED_BASE_FILE_NAME: &str = "provider-catalog-base.json";
/// Subdirectorio XDG donde vive el estado de providers.
pub const XDG_PROVIDERS_SUBDIR: &str = "nexum/providers";

/// `$XDG_DATA_HOME` (o `$HOME/.local/share`). `None` sin entorno.
pub fn data_home() -> Option<PathBuf> {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok()
}

/// Directorio XDG del estado de providers. Es el que `reconcile` escribe y el
/// que los fixtures tienen que poblar.
pub fn providers_dir() -> Option<PathBuf> {
    data_home().map(|d| d.join(XDG_PROVIDERS_SUBDIR))
}

/// Catálogo vivo: lo que `reconcile` publica y la única fuente con datos
/// frescos (credenciales verificadas, modelos descubiertos, usable_now real).
pub fn live_catalog() -> Option<PathBuf> {
    providers_dir().map(|d| d.join(LIVE_CATALOG_FILE_NAME))
}

/// Snapshot previo, para cuando el vivo quedó corrupto a mitad de escritura.
pub fn previous_catalog() -> Option<PathBuf> {
    providers_dir().map(|d| d.join(PREVIOUS_CATALOG_FILE_NAME))
}

/// Catálogo base del slot instalado, hermano del ejecutable.
///
/// Es una copia congelada del base al empaquetar: sirve como último recurso
/// (existe siempre, no depende de que `reconcile` haya corrido) pero **no**
/// tiene estado vivo. Quien lo lea creyendo que sí, miente.
pub fn installed_base() -> Option<PathBuf> {
    if isolated() {
        return None;
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|slot| slot.join(INSTALLED_BASE_FILE_NAME)))
}

/// Variable que apaga TODO fallback fuera de XDG.
///
/// Existe porque el aislamiento por `XDG_DATA_HOME` solo, que es lo que hacían
/// los fixtures, no aísla nada: apunta el nivel vivo a un temp vacío y el
/// resolver sigue bajando la cadena hasta encontrar un catálogo real de la
/// máquina. Un test que cree estar sin catálogo termina leyendo el estado del
/// desarrollador y pasa o falla según qué providers tenga conectados.
///
/// No alcanza con `cfg(test)`: cuando `nexum-tui` corre sus tests, este crate
/// se compila como dependencia normal y su `cfg(test)` está apagado. Tiene que
/// ser una señal en runtime que cruce el límite de crate.
pub const ISOLATED_ENV: &str = "NEXUM_CATALOG_ISOLATED";

fn isolated() -> bool {
    std::env::var(ISOLATED_ENV)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Base del checkout, sólo en builds de desarrollo y nunca bajo aislamiento.
///
/// Es el nivel más peligroso de la cadena: existe siempre que haya un checkout,
/// así que convierte "no hay catálogo" en "hay uno que nadie pidió". Es la
/// misma trampa que el path hardcodeado de `/tmp` en la estampa, una capa más
/// arriba.
pub fn checkout_base() -> Option<PathBuf> {
    if isolated() || !cfg!(debug_assertions) {
        return None;
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/provider-catalog-base.json");
    p.is_file().then_some(p)
}

/// De dónde salió el catálogo que se está leyendo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSource {
    /// Vivo y válido: el único con datos frescos.
    Live,
    /// El vivo no servía; se usó el snapshot anterior.
    Previous,
    /// Ni vivo ni snapshot; base congelada del slot. Sin estado vivo.
    Base,
    /// Base del checkout (sólo dev).
    Checkout,
    /// No hay ninguno.
    Missing,
}

/// Resolución completa, con procedencia.
#[derive(Debug, Clone)]
pub struct CatalogResolution {
    pub path: PathBuf,
    pub source: CatalogSource,
    /// El vivo existía pero no parseaba. Se anota para que el diagnóstico no
    /// diga "no hay catálogo" cuando lo que hay es uno roto.
    pub live_rejected: bool,
}

fn parses(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .is_some()
}

/// Resolución productiva, en orden de frescura: vivo válido → snapshot previo
/// válido → base instalada → base del checkout (sólo dev).
///
/// Nunca resuelve relativo al cwd: el producto instalado no depende de desde
/// dónde se lo invoque.
pub fn resolve() -> CatalogResolution {
    let live = live_catalog();
    let live_rejected = live.as_deref().is_some_and(|p| p.is_file() && !parses(p));

    if let Some(p) = live.as_deref().filter(|p| p.is_file() && parses(p)) {
        return CatalogResolution {
            path: p.to_path_buf(),
            source: CatalogSource::Live,
            live_rejected,
        };
    }
    if let Some(p) = previous_catalog()
        .as_deref()
        .filter(|p| p.is_file() && parses(p))
    {
        return CatalogResolution {
            path: p.to_path_buf(),
            source: CatalogSource::Previous,
            live_rejected,
        };
    }
    if let Some(p) = installed_base().as_deref().filter(|p| p.is_file()) {
        return CatalogResolution {
            path: p.to_path_buf(),
            source: CatalogSource::Base,
            live_rejected,
        };
    }
    if let Some(p) = checkout_base() {
        return CatalogResolution {
            path: p,
            source: CatalogSource::Checkout,
            live_rejected,
        };
    }
    CatalogResolution {
        path: live.unwrap_or_else(|| PathBuf::from(LIVE_CATALOG_FILE_NAME)),
        source: CatalogSource::Missing,
        live_rejected,
    }
}

/// `XDG_DATA_HOME` es estado global del proceso: todos los tests del crate que
/// lo tocan (catalog_path_test, free_access, generación, tool_support) deben
/// serializarse sobre el MISMO lock. Antes cada módulo usaba el suyo y en
/// Windows dos tests podían pisarse la variable entre sí.
#[cfg(test)]
pub(crate) static XDG_DATA_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn with_xdg<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let _g = XDG_DATA_HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let previo = std::env::var("XDG_DATA_HOME").ok();
    std::env::set_var("XDG_DATA_HOME", dir);
    let out = f();
    match previo {
        Some(v) => std::env::set_var("XDG_DATA_HOME", v),
        None => std::env::remove_var("XDG_DATA_HOME"),
    }
    out
}

#[cfg(test)]
#[path = "catalog_path_test.rs"]
mod tests;
