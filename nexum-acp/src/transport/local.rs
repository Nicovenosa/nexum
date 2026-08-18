//! Cliente ACP local para el socket Unix del host durable.
//!
//! No redefine framing: delega toda la serializacion y el backpressure a
//! [`UnixTransport`]. Solo agrega conexion y validacion de disponibilidad.

use std::{
    ffi::OsStr,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use tokio::{net::UnixStream, time::timeout};

use super::{
    types::{AcpError, IncomingMessage, RequestId},
    unix::{UnixTransport, LOCAL_PROTOCOL_VERSION},
    AcpTransport,
};

/// Error de conexion local sin incluir parametros ACP ni datos de usuario.
#[derive(Debug)]
pub enum LocalTransportError {
    Connect(std::io::Error),
    TimedOut,
    HostIdentity(String),
    Handshake(AcpError),
    Protocol(String),
    NotReady(String),
}

impl fmt::Display for LocalTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(error) => write!(f, "local ACP connection failed: {error}"),
            Self::TimedOut => f.write_str("local ACP readiness timed out"),
            Self::HostIdentity(reason) => write!(f, "local ACP host identity rejected: {reason}"),
            Self::Handshake(error) => write!(f, "local ACP handshake failed: {}", error.message),
            Self::Protocol(reason) => write!(f, "local ACP protocol error: {reason}"),
            Self::NotReady(state) => write!(f, "local ACP runtime is not ready: {state}"),
        }
    }
}

impl std::error::Error for LocalTransportError {}

impl LocalTransportError {
    /// Only an absent or refused socket permits `auto` to start a host. A
    /// reachable host with an incompatible protocol must be surfaced as-is.
    pub fn permits_auto_start(&self) -> bool {
        matches!(
            self,
            Self::Connect(error)
                if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
        )
    }

    pub fn is_protocol_failure(&self) -> bool {
        matches!(
            self,
            Self::HostIdentity(_) | Self::Handshake(_) | Self::Protocol(_)
        )
    }
}

/// Error estructurado de resolución del runtime ACP. Ningún variant permite
/// seguir hacia operaciones de socket, proceso o recuperación de hosts.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeDirectoryError {
    #[error("INVALID_RUNTIME_DIRECTORY: XDG_RUNTIME_DIR is empty")]
    EmptyRuntimeDirectory,
    #[error("RUNTIME_DIRECTORY_NOT_ABSOLUTE: {path}")]
    NotAbsolute { path: PathBuf },
    #[error("RUNTIME_DIRECTORY_NOT_FOUND: {path}")]
    NotFound { path: PathBuf },
    #[error("RUNTIME_DIRECTORY_NOT_DIRECTORY: {path}")]
    NotDirectory { path: PathBuf },
    #[error("RUNTIME_DIRECTORY_SYMLINK: {path}")]
    Symlink { path: PathBuf },
    #[error(
        "RUNTIME_DIRECTORY_OWNER_MISMATCH: {path}; expected uid {expected_uid}, found {actual_uid}"
    )]
    OwnerMismatch {
        path: PathBuf,
        expected_uid: u32,
        actual_uid: u32,
    },
    #[error("RUNTIME_DIRECTORY_UNSAFE_PERMISSIONS: {path}; mode {mode:o}")]
    UnsafePermissions { path: PathBuf, mode: u32 },
    #[error("RUNTIME_DIRECTORY_CANONICALIZE_FAILED: {path}: {source}")]
    Canonicalize { path: PathBuf, source: io::Error },
    #[error("INVALID_RUNTIME_DIRECTORY: could not inspect {path}: {source}")]
    Metadata { path: PathBuf, source: io::Error },
    #[error("INVALID_RUNTIME_DIRECTORY: no supported home directory is available")]
    HomeUnavailable,
    #[error("INVALID_RUNTIME_GUARD: {reason}")]
    InvalidGuard { reason: String },
    #[error("RUNTIME_SCOPE_VIOLATION: runtime {runtime} is outside guard {guard}")]
    ScopeViolation { runtime: PathBuf, guard: PathBuf },
}

#[derive(Debug, Clone, Copy)]
struct RuntimeMetadata {
    is_directory: bool,
    is_symlink: bool,
    uid: u32,
    mode: u32,
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn effective_uid() -> u32 {
    0
}

fn validate_runtime_metadata(
    path: &Path,
    metadata: RuntimeMetadata,
    expected_uid: u32,
) -> Result<(), RuntimeDirectoryError> {
    if metadata.is_symlink {
        return Err(RuntimeDirectoryError::Symlink {
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_directory {
        return Err(RuntimeDirectoryError::NotDirectory {
            path: path.to_path_buf(),
        });
    }
    if metadata.uid != expected_uid {
        return Err(RuntimeDirectoryError::OwnerMismatch {
            path: path.to_path_buf(),
            expected_uid,
            actual_uid: metadata.uid,
        });
    }
    if metadata.mode & 0o077 != 0 {
        return Err(RuntimeDirectoryError::UnsafePermissions {
            path: path.to_path_buf(),
            mode: metadata.mode,
        });
    }
    Ok(())
}

fn runtime_metadata(path: &Path) -> Result<RuntimeMetadata, RuntimeDirectoryError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err(RuntimeDirectoryError::NotFound {
                path: path.to_path_buf(),
            })
        }
        Err(source) => {
            return Err(RuntimeDirectoryError::Metadata {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    #[cfg(unix)]
    {
        Ok(RuntimeMetadata {
            is_directory: metadata.is_dir(),
            is_symlink: metadata.file_type().is_symlink(),
            uid: metadata.uid(),
            mode: metadata.mode(),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(RuntimeMetadata {
            is_directory: metadata.is_dir(),
            is_symlink: metadata.file_type().is_symlink(),
            uid: effective_uid(),
            mode: 0,
        })
    }
}

fn validate_runtime_directory(path: &Path) -> Result<PathBuf, RuntimeDirectoryError> {
    if !path.is_absolute() {
        return Err(RuntimeDirectoryError::NotAbsolute {
            path: path.to_path_buf(),
        });
    }
    validate_runtime_metadata(path, runtime_metadata(path)?, effective_uid())?;
    fs::canonicalize(path).map_err(|source| RuntimeDirectoryError::Canonicalize {
        path: path.to_path_buf(),
        source,
    })
}

fn invalid_guard(error: RuntimeDirectoryError) -> RuntimeDirectoryError {
    RuntimeDirectoryError::InvalidGuard {
        reason: error.to_string(),
    }
}

fn resolve_runtime_root(
    runtime_value: Option<&OsStr>,
    guard_value: Option<&OsStr>,
) -> Result<PathBuf, RuntimeDirectoryError> {
    let runtime = match runtime_value {
        Some(value) if value.is_empty() => {
            return Err(RuntimeDirectoryError::EmptyRuntimeDirectory)
        }
        Some(value) => PathBuf::from(value),
        None => {
            let path = dirs_next::home_dir()
                .ok_or(RuntimeDirectoryError::HomeUnavailable)?
                .join(".nexum/runtime");
            let parent = path
                .parent()
                .ok_or(RuntimeDirectoryError::HomeUnavailable)?;
            fs::create_dir_all(parent).map_err(|source| RuntimeDirectoryError::Metadata {
                path: parent.to_path_buf(),
                source,
            })?;
            ensure_private_runtime_directory(&path)?;
            path
        }
    };
    let runtime = validate_runtime_directory(&runtime)?;

    if let Some(value) = guard_value {
        if value.is_empty() {
            return Err(RuntimeDirectoryError::InvalidGuard {
                reason: "NEXUM_RUNTIME_ROOT_GUARD is empty".to_string(),
            });
        }
        let guard = validate_runtime_directory(Path::new(value)).map_err(invalid_guard)?;
        if runtime != guard && !runtime.starts_with(&guard) {
            return Err(RuntimeDirectoryError::ScopeViolation { runtime, guard });
        }
    }
    Ok(runtime)
}

/// Directorio privado compartido por el host y sus clientes locales. La
/// resolución valida primero XDG y el guard de aislamiento; por ello ningún
/// consumidor recibe una ruta de socket ante un entorno inseguro.
pub fn local_runtime_directory() -> Result<PathBuf, RuntimeDirectoryError> {
    let root = resolve_runtime_root(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        std::env::var_os("NEXUM_RUNTIME_ROOT_GUARD").as_deref(),
    )?;
    // Bajo test, cada proceso tiene SU directorio: los tests de bootstrap
    // spawnean hosts reales y bindean sockets, y compartir el directorio los
    // hacía competir por el mismo `acp.sock`. El síntoma era un flake que
    // cambiaba de nombre entre corridas — tres tests distintos, mismo helper.
    //
    // Los sockets son estado de SESIÓN, así que el PID corresponde. `cron.db`
    // vivía acá y NO podía ir por PID: se movió antes, precisamente porque
    // aislar en bloque sin mirar la vida de cada dato habría dejado al usuario
    // sin tareas programadas.
    let runtime = if nexum_agent::sandbox::running_under_test() {
        root.join(format!("nexum-{}", nexum_agent::sandbox::session_suffix()))
    } else {
        root.join("nexum")
    };
    ensure_private_runtime_directory(&runtime)?;
    let runtime = validate_runtime_directory(&runtime)?;
    if let Some(guard) = std::env::var_os("NEXUM_RUNTIME_ROOT_GUARD") {
        let guard = validate_runtime_directory(Path::new(&guard)).map_err(invalid_guard)?;
        if runtime != guard && !runtime.starts_with(&guard) {
            return Err(RuntimeDirectoryError::ScopeViolation { runtime, guard });
        }
    }
    Ok(runtime)
}

pub fn default_local_socket_path() -> Result<PathBuf, RuntimeDirectoryError> {
    Ok(local_runtime_directory()?.join("acp.sock"))
}

pub fn ensure_private_runtime_directory(path: &Path) -> Result<(), RuntimeDirectoryError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|source| RuntimeDirectoryError::Metadata {
                path: path.to_path_buf(),
                source,
            })?;
        }
        Err(source) => {
            return Err(RuntimeDirectoryError::Metadata {
                path: path.to_path_buf(),
                source,
            })
        }
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        RuntimeDirectoryError::Metadata {
            path: path.to_path_buf(),
            source,
        }
    })?;
    validate_runtime_directory(path).map(|_| ())
}

/// Cliente conectado al host ACP local.
///
/// Al descartarse el ultimo `Arc`, `UnixTransport` cierra ambas mitades del
/// stream y sus loops acotados terminan; nunca controla el proceso del host.
pub struct LocalAcpTransport {
    socket_path: PathBuf,
    inner: Arc<UnixTransport>,
}

impl fmt::Debug for LocalAcpTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalAcpTransport")
            .field("socket_path", &self.socket_path)
            .finish_non_exhaustive()
    }
}

impl LocalAcpTransport {
    /// Conecta y verifica que el servidor use el framing esperado y ya acepte
    /// requests. El timeout cubre tanto `connect` como el RPC `health`.
    pub async fn connect_ready(
        path: impl AsRef<Path>,
        budget: Duration,
    ) -> Result<Self, LocalTransportError> {
        Self::connect_ready_guarded(path, budget, |_| Ok(())).await
    }

    /// Connects once, validates the identity of the peer on that exact
    /// [`UnixStream`], and only then constructs the RPC transport and sends the
    /// readiness handshake. A rejected stream is dropped without sending bytes.
    pub async fn connect_ready_guarded<F>(
        path: impl AsRef<Path>,
        budget: Duration,
        verify_peer: F,
    ) -> Result<Self, LocalTransportError>
    where
        F: FnOnce(&UnixStream) -> Result<(), String>,
    {
        let socket_path = path.as_ref().to_path_buf();
        timeout(budget, async {
            let stream = UnixStream::connect(&socket_path)
                .await
                .map_err(LocalTransportError::Connect)?;
            verify_peer(&stream).map_err(LocalTransportError::HostIdentity)?;
            let transport = Self {
                socket_path,
                inner: Arc::new(UnixTransport::from_stream(stream)),
            };
            transport.verify_ready().await?;
            Ok(transport)
        })
        .await
        .map_err(|_| LocalTransportError::TimedOut)?
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    async fn verify_ready(&self) -> Result<(), LocalTransportError> {
        let health = self
            .inner
            .send_request("health", serde_json::json!({}))
            .await
            .map_err(LocalTransportError::Handshake)?;
        let version = health
            .get("protocol_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| LocalTransportError::Protocol("missing protocol_version".to_string()))?;
        if version != u64::from(LOCAL_PROTOCOL_VERSION) {
            return Err(LocalTransportError::Protocol(format!(
                "expected version {LOCAL_PROTOCOL_VERSION}, got {version}"
            )));
        }
        if health
            .get("runtime_available")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        {
            return Err(LocalTransportError::Protocol(
                "runtime_available is not true".to_string(),
            ));
        }
        match health.get("health").and_then(serde_json::Value::as_str) {
            Some("ready") => Ok(()),
            Some(state) => Err(LocalTransportError::NotReady(state.to_string())),
            None => Err(LocalTransportError::Protocol(
                "missing health state".to_string(),
            )),
        }
    }
}

#[async_trait::async_trait]
impl AcpTransport for LocalAcpTransport {
    async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AcpError> {
        self.inner.send_request(method, params).await
    }

    async fn send_notification(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), AcpError> {
        self.inner.send_notification(method, params).await
    }

    async fn recv(&self) -> Option<IncomingMessage> {
        self.inner.recv().await
    }

    async fn send_response(
        &self,
        id: RequestId,
        result: Result<serde_json::Value, AcpError>,
    ) -> Result<(), AcpError> {
        self.inner.send_response(id, result).await
    }

    async fn close(&self) -> Result<(), AcpError> {
        self.inner.close().await
    }
}

#[cfg(test)]
mod runtime_directory_tests {
    use std::{ffi::OsStr, fs, os::unix::fs::PermissionsExt, path::Path};

    use super::{
        resolve_runtime_root, validate_runtime_metadata, RuntimeDirectoryError, RuntimeMetadata,
    };

    fn secure(path: &Path) {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn runtime_dir_accepts_owned_0700_directory() {
        let temp = tempfile::tempdir().unwrap();
        secure(temp.path());
        assert_eq!(
            resolve_runtime_root(Some(temp.path().as_os_str()), None).unwrap(),
            fs::canonicalize(temp.path()).unwrap()
        );
    }

    #[test]
    fn runtime_dir_rejects_empty_value() {
        assert!(matches!(
            resolve_runtime_root(Some(OsStr::new("")), None),
            Err(RuntimeDirectoryError::EmptyRuntimeDirectory)
        ));
    }

    #[test]
    fn runtime_dir_rejects_relative_path() {
        assert!(matches!(
            resolve_runtime_root(Some(OsStr::new("relative")), None),
            Err(RuntimeDirectoryError::NotAbsolute { .. })
        ));
    }

    #[test]
    fn runtime_dir_rejects_missing_path() {
        let missing = tempfile::tempdir().unwrap().path().join("missing");
        assert!(matches!(
            resolve_runtime_root(Some(missing.as_os_str()), None),
            Err(RuntimeDirectoryError::NotFound { .. })
        ));
    }

    #[test]
    fn runtime_dir_rejects_regular_file() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("runtime-file");
        fs::write(&file, "not a directory").unwrap();
        assert!(matches!(
            resolve_runtime_root(Some(file.as_os_str()), None),
            Err(RuntimeDirectoryError::NotDirectory { .. })
        ));
    }

    #[test]
    fn runtime_dir_rejects_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        secure(&target);
        let link = temp.path().join("runtime-link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(
            resolve_runtime_root(Some(link.as_os_str()), None),
            Err(RuntimeDirectoryError::Symlink { .. })
        ));
    }

    #[test]
    fn runtime_dir_rejects_group_permissions() {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o750)).unwrap();
        assert!(matches!(
            resolve_runtime_root(Some(temp.path().as_os_str()), None),
            Err(RuntimeDirectoryError::UnsafePermissions { .. })
        ));
    }

    #[test]
    fn runtime_dir_rejects_world_permissions() {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o707)).unwrap();
        assert!(matches!(
            resolve_runtime_root(Some(temp.path().as_os_str()), None),
            Err(RuntimeDirectoryError::UnsafePermissions { .. })
        ));
    }

    #[test]
    fn runtime_dir_rejects_owner_mismatch() {
        let metadata = RuntimeMetadata {
            is_directory: true,
            is_symlink: false,
            uid: 42,
            mode: 0o700,
        };
        assert!(matches!(
            validate_runtime_metadata(Path::new("/runtime"), metadata, 7),
            Err(RuntimeDirectoryError::OwnerMismatch { .. })
        ));
    }

    #[test]
    fn runtime_guard_accepts_exact_root() {
        let temp = tempfile::tempdir().unwrap();
        secure(temp.path());
        assert!(
            resolve_runtime_root(Some(temp.path().as_os_str()), Some(temp.path().as_os_str()))
                .is_ok()
        );
    }

    #[test]
    fn runtime_guard_accepts_real_descendant() {
        let temp = tempfile::tempdir().unwrap();
        secure(temp.path());
        let child = temp.path().join("isolated");
        fs::create_dir(&child).unwrap();
        secure(&child);
        assert!(
            resolve_runtime_root(Some(child.as_os_str()), Some(temp.path().as_os_str())).is_ok()
        );
    }

    #[test]
    fn runtime_guard_rejects_sibling_prefix_collision() {
        let parent = tempfile::tempdir().unwrap();
        secure(parent.path());
        let guard = parent.path().join("guard");
        let sibling = parent.path().join("guard-foo");
        fs::create_dir(&guard).unwrap();
        fs::create_dir(&sibling).unwrap();
        secure(&guard);
        secure(&sibling);
        assert!(matches!(
            resolve_runtime_root(Some(sibling.as_os_str()), Some(guard.as_os_str())),
            Err(RuntimeDirectoryError::ScopeViolation { .. })
        ));
    }

    #[test]
    fn runtime_guard_rejects_path_outside_root() {
        let guard = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        secure(guard.path());
        secure(runtime.path());
        assert!(matches!(
            resolve_runtime_root(
                Some(runtime.path().as_os_str()),
                Some(guard.path().as_os_str())
            ),
            Err(RuntimeDirectoryError::ScopeViolation { .. })
        ));
    }

    #[test]
    fn runtime_guard_rejects_invalid_guard() {
        let runtime = tempfile::tempdir().unwrap();
        secure(runtime.path());
        assert!(matches!(
            resolve_runtime_root(Some(runtime.path().as_os_str()), Some(OsStr::new(""))),
            Err(RuntimeDirectoryError::InvalidGuard { .. })
        ));
    }

    #[test]
    fn invalid_explicit_runtime_has_no_fallback() {
        let missing = tempfile::tempdir().unwrap().path().join("missing");
        assert!(matches!(
            resolve_runtime_root(Some(missing.as_os_str()), None),
            Err(RuntimeDirectoryError::NotFound { .. })
        ));
    }
}
