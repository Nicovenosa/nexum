//! Resolver ÚNICO del directorio de config/datos de Nexum.
//! ADR-NEXUM-IDENTITY-MIGRATION D3: `~/.nexum` es el store activo;
//! `~/.peri` queda como legacy INTACTO (migración por copia, una vez,
//! nunca destructiva). Toda escritura nueva va al store nuevo.

use std::path::{Path, PathBuf};

const NEW_DIR: &str = ".nexum";
const LEGACY_DIR: &str = ".peri";
const MARKER: &str = ".migrated-from-peri";

/// Directorio home de config de Nexum (`~/.nexum`), con migración
/// automática desde `~/.peri` la primera vez (copia, no move).
pub fn nexum_home() -> PathBuf {
    let home = dirs_next::home_dir().unwrap_or_else(|| PathBuf::from("."));
    resolve_store(&home)
}

/// Store de workspace (`<cwd>/.nexum`), con la misma migración desde
/// `<cwd>/.peri` (settings de proyecto).
pub fn nexum_workspace_dir(cwd: &Path) -> PathBuf {
    resolve_store(cwd)
}

fn resolve_store(base: &Path) -> PathBuf {
    let new = base.join(NEW_DIR);
    let legacy = base.join(LEGACY_DIR);
    if legacy.is_dir() && !new.join(MARKER).exists() {
        // Migración única (marker-gated): copiar lo FALTANTE (settings,
        // threads.db, …) sin tocar el origen ni pisar lo existente.
        // Cubre también el caso ~/.nexum preexistente (lo usa el core
        // Python: chroma/, adaptive.json) — merge if-absent, jamás clobber.
        let _ = copy_dir_recursive(&legacy, &new);
        let _ = std::fs::write(new.join(MARKER), "migrated from ~/.peri (kept intact)\n");
        restrict_dir_permissions(&new);
    } else if !new.exists() {
        let _ = std::fs::create_dir_all(&new);
        restrict_dir_permissions(&new);
    }
    new
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else if ty.is_file() && !to.exists() {
            std::fs::copy(entry.path(), &to)?;
            // Conservar restricción de archivos sensibles (0600 si el
            // origen era restrictivo; settings/tokens siempre 0600).
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(entry.path()) {
                    // Owner-only: se conserva el modo original recortado a
                    // 0o700 (nunca group/other en el store nuevo).
                    let mode = meta.permissions().mode() & 0o700;
                    let _ = std::fs::set_permissions(
                        &to,
                        std::fs::Permissions::from_mode(mode),
                    );
                }
            }
        }
    }
    Ok(())
}

fn restrict_dir_permissions(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
}

/// Base de cron: dato PERSISTENTE del usuario, con nombre estable.
pub fn cron_store_path() -> PathBuf {
    nexum_home().join("cron.db")
}

/// Migra la base de cron desde el runtime dir volátil, si hace falta.
///
/// Vive acá y no en `nexum-acp-host` porque **la migración no puede depender de
/// que arranque el host**. El host lo spawnea la TUI, así que un usuario que no
/// abra la TUI antes de cerrar sesión perdía las tareas igual — el arreglo
/// original tapaba el agujero sólo para quien pasara por ese camino.
///
/// Es idempotente y barata: si el destino existe, retorna sin tocar nada. Por
/// eso se puede llamar desde varias entradas sin coordinarlas.
pub fn migrate_cron_store_if_needed() {
    let destino = cron_store_path();
    if destino.exists() {
        return;
    }
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) else {
        return;
    };
    let origen = runtime.join("nexum").join("cron.db");
    if !origen.is_file() {
        return;
    }
    if let Some(parent) = destino.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    // Copia, no movimiento: si algo sale mal el original sigue ahí hasta el
    // próximo logout.
    match std::fs::copy(&origen, &destino) {
        Ok(bytes) => tracing::info!(
            desde = %origen.display(), hacia = %destino.display(), bytes,
            "base de cron migrada del runtime dir (volátil) al home de Nexum"
        ),
        Err(e) => tracing::warn!(error = %e, "no se pudo migrar la base de cron"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migra_desde_legacy_sin_borrar_origen() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join(".peri");
        std::fs::create_dir_all(legacy.join("threads")).unwrap();
        std::fs::write(legacy.join("settings.json"), "{\"a\":1}").unwrap();
        std::fs::write(legacy.join("threads/threads.db"), "DBDATA").unwrap();
        let new = resolve_store(tmp.path());
        assert_eq!(new, tmp.path().join(".nexum"));
        assert_eq!(
            std::fs::read_to_string(new.join("settings.json")).unwrap(),
            "{\"a\":1}"
        );
        assert_eq!(
            std::fs::read_to_string(new.join("threads/threads.db")).unwrap(),
            "DBDATA"
        );
        assert!(new.join(MARKER).exists(), "marker de migración presente");
        // El origen queda INTACTO (regla dura: no borrar ~/.peri).
        assert!(legacy.join("settings.json").exists());
        assert!(legacy.join("threads/threads.db").exists());
    }

    #[test]
    fn test_usa_nexum_si_ya_existe_sin_re_migrar() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".nexum")).unwrap();
        std::fs::write(tmp.path().join(".nexum/settings.json"), "nuevo").unwrap();
        let legacy = tmp.path().join(".peri");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("settings.json"), "viejo").unwrap();
        let new = resolve_store(tmp.path());
        assert_eq!(
            std::fs::read_to_string(new.join("settings.json")).unwrap(),
            "nuevo",
            "si ~/.nexum existe, NO se pisa con legacy"
        );
    }

    #[test]
    fn test_crea_nuevo_sin_legacy_con_permisos() {
        let tmp = tempfile::tempdir().unwrap();
        let new = resolve_store(tmp.path());
        assert!(new.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&new).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "dir 0700");
        }
    }
}
