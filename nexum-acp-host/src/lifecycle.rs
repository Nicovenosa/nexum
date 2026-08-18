use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use tokio::net::{UnixListener, UnixStream};

pub fn default_socket_path() -> Result<PathBuf, nexum_acp::transport::local::RuntimeDirectoryError>
{
    Ok(socket_path_in_runtime(
        &nexum_acp::transport::local::local_runtime_directory()?,
    ))
}

pub fn default_cron_store_path(
) -> Result<PathBuf, nexum_acp::transport::local::RuntimeDirectoryError> {
    // Migra lo que haya quedado en el runtime dir antes de devolver la ruta
    // nueva: apuntar al lugar bueno sin traerse las tareas existentes las haría
    // desaparecer por nuestra mano.
    nexum_agent::config_home::migrate_cron_store_if_needed();
    Ok(cron_store_path())
}

pub fn socket_path_in_runtime(runtime: &Path) -> PathBuf {
    runtime.join("acp.sock")
}

/// Ubicación heredada de la base de cron: el runtime dir.
///
/// `XDG_RUNTIME_DIR` es VOLÁTIL por definición — el sistema lo borra al cerrar
/// sesión. Las tareas programadas del usuario se estaban perdiendo en cada
/// logout sin que nadie avisara. Se conserva sólo para migrar lo que haya.
pub fn legacy_cron_store_path_in_runtime(runtime: &Path) -> PathBuf {
    runtime.join("cron.db")
}

pub fn cron_store_path() -> PathBuf {
    nexum_agent::config_home::cron_store_path()
}


pub async fn bind(path: &PathBuf) -> anyhow::Result<UnixListener> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("socket has no parent"))?;
    nexum_acp::transport::local::ensure_private_runtime_directory(parent)?;

    if path.exists() {
        match UnixStream::connect(path).await {
            Ok(_) => {
                return Err(
                    io::Error::new(io::ErrorKind::AddrInUse, "ACP host already running").into(),
                )
            }
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                fs::remove_file(path)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let listener = UnixListener::bind(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[cfg(unix)]
pub fn socket_inode(path: &Path) -> io::Result<u64> {
    Ok(fs::symlink_metadata(path)?.ino())
}

#[cfg(unix)]
pub fn remove_owned_socket(path: &Path, expected_inode: u64) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.ino() == expected_inode => {
            fs::remove_file(path)?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
