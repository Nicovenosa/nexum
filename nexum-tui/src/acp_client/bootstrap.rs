use std::{
    fmt,
    fs::{self, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use nexum_acp::transport::local::{
    default_local_socket_path, ensure_private_runtime_directory, LocalAcpTransport,
    LocalTransportError, RuntimeDirectoryError,
};
use nexum_acp::transport::{
    types::{AcpError, IncomingMessage, RequestId},
    AcpTransport,
};

use super::AcpClientTransport;

const READY_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostIdentityReasonCode {
    Match,
    SlotMismatch,
    PeerPidUnavailable,
    ExecutableUnresolved,
    PathInvalid,
    PeerExited,
    IdentityAmbiguous,
}

impl HostIdentityReasonCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Match => "HOST_IDENTITY_MATCH",
            Self::SlotMismatch => "HOST_SLOT_MISMATCH",
            Self::PeerPidUnavailable => "HOST_PEER_PID_UNAVAILABLE",
            Self::ExecutableUnresolved => "HOST_EXECUTABLE_UNRESOLVED",
            Self::PathInvalid => "HOST_PATH_INVALID",
            Self::PeerExited => "HOST_PEER_EXITED",
            Self::IdentityAmbiguous => "HOST_IDENTITY_AMBIGUOUS",
        }
    }
}

impl fmt::Display for HostIdentityReasonCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostIdentityDiagnostic {
    pub socket_path: PathBuf,
    pub peer_pid: Option<u32>,
    pub observed_executable: Option<PathBuf>,
    pub expected_slot: PathBuf,
    pub observed_slot: Option<PathBuf>,
    pub guard_result: &'static str,
    pub reason_code: HostIdentityReasonCode,
}

impl fmt::Display for HostIdentityDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "socket_path={} peer_pid={} observed_executable={} expected_slot={} \
             observed_slot={} guard_result={} reason_code={}",
            self.socket_path.display(),
            self.peer_pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "unavailable".to_string()),
            self.observed_executable
                .as_deref()
                .map(Path::display)
                .map(|path| path.to_string())
                .unwrap_or_else(|| "unresolved".to_string()),
            self.expected_slot.display(),
            self.observed_slot
                .as_deref()
                .map(Path::display)
                .map(|path| path.to_string())
                .unwrap_or_else(|| "unresolved".to_string()),
            self.guard_result,
            self.reason_code,
        )
    }
}

#[derive(Debug)]
pub(crate) struct HostIdentityError {
    diagnostic: Box<HostIdentityDiagnostic>,
}

impl HostIdentityError {
    #[cfg(test)]
    pub(crate) fn reason_code(&self) -> HostIdentityReasonCode {
        self.diagnostic.reason_code
    }

    #[cfg(test)]
    pub(crate) fn diagnostic(&self) -> &HostIdentityDiagnostic {
        &self.diagnostic
    }
}

impl fmt::Display for HostIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic.fmt(formatter)
    }
}

impl std::error::Error for HostIdentityError {}

pub(crate) trait HostIdentitySource {
    fn peer_pid(&self, stream: &tokio::net::UnixStream) -> io::Result<Option<u32>>;
    fn executable_for_pid(&self, pid: u32) -> io::Result<PathBuf>;
    fn peer_exists(&self, pid: u32) -> bool;
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;
}

struct LinuxHostIdentitySource;

impl HostIdentitySource for LinuxHostIdentitySource {
    fn peer_pid(&self, stream: &tokio::net::UnixStream) -> io::Result<Option<u32>> {
        #[cfg(target_os = "linux")]
        {
            stream
                .peer_cred()?
                .pid()
                .map(|pid| {
                    u32::try_from(pid)
                        .map_err(|_| io::Error::other("peer PID is outside the valid range"))
                })
                .transpose()
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = stream;
            Ok(None)
        }
    }

    fn executable_for_pid(&self, pid: u32) -> io::Result<PathBuf> {
        fs::read_link(format!("/proc/{pid}/exe"))
    }

    fn peer_exists(&self, pid: u32) -> bool {
        PathBuf::from(format!("/proc/{pid}")).is_dir()
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }
}

/// Fail-closed binding between the connected Unix peer and the exact
/// `nexum-acp-host` executable in the TUI's canonical runtime slot.
#[derive(Debug, Clone)]
pub(crate) struct HostIdentityGuard {
    expected_executable: PathBuf,
    expected_slot: PathBuf,
    last_rejection: Arc<Mutex<Option<HostIdentityDiagnostic>>>,
}

impl HostIdentityGuard {
    pub(crate) fn for_current_runtime() -> anyhow::Result<Self> {
        let tui_executable =
            std::env::current_exe().map_err(|_| anyhow::anyhow!("HOST_PATH_INVALID"))?;
        let canonical_tui =
            fs::canonicalize(tui_executable).map_err(|_| anyhow::anyhow!("HOST_PATH_INVALID"))?;
        let slot = canonical_tui
            .parent()
            .ok_or_else(|| anyhow::anyhow!("HOST_IDENTITY_AMBIGUOUS"))?;
        Self::for_expected_host(&slot.join("nexum-acp-host"))
    }

    pub(crate) fn for_expected_host(expected_executable: &Path) -> anyhow::Result<Self> {
        let expected_slot = expected_executable
            .parent()
            .ok_or_else(|| anyhow::anyhow!("HOST_IDENTITY_AMBIGUOUS"))?;
        let expected_slot =
            fs::canonicalize(expected_slot).map_err(|_| anyhow::anyhow!("HOST_PATH_INVALID"))?;
        let expected_executable = fs::canonicalize(expected_executable)
            .map_err(|_| anyhow::anyhow!("HOST_PATH_INVALID"))?;
        if expected_executable.parent() != Some(expected_slot.as_path()) {
            anyhow::bail!("HOST_SLOT_MISMATCH");
        }
        Ok(Self {
            expected_executable,
            expected_slot,
            last_rejection: Arc::new(Mutex::new(None)),
        })
    }

    pub(crate) fn expected_executable(&self) -> &Path {
        &self.expected_executable
    }

    fn verify(
        &self,
        socket_path: &Path,
        stream: &tokio::net::UnixStream,
    ) -> Result<(), HostIdentityError> {
        self.verify_with_source(socket_path, stream, &LinuxHostIdentitySource)
    }

    pub(crate) fn verify_with_source(
        &self,
        socket_path: &Path,
        stream: &tokio::net::UnixStream,
        source: &dyn HostIdentitySource,
    ) -> Result<(), HostIdentityError> {
        let peer_pid = match source.peer_pid(stream) {
            Ok(Some(pid)) if pid > 0 => pid,
            Ok(_) | Err(_) => {
                return Err(self.rejection(
                    socket_path,
                    None,
                    None,
                    None,
                    HostIdentityReasonCode::PeerPidUnavailable,
                ));
            }
        };

        let observed_link = match source.executable_for_pid(peer_pid) {
            Ok(path) => path,
            Err(_) => {
                let reason = if source.peer_exists(peer_pid) {
                    HostIdentityReasonCode::ExecutableUnresolved
                } else {
                    HostIdentityReasonCode::PeerExited
                };
                return Err(self.rejection(socket_path, Some(peer_pid), None, None, reason));
            }
        };
        let observed_executable = match source.canonicalize(&observed_link) {
            Ok(path) => path,
            Err(_) => {
                let reason = if source.peer_exists(peer_pid) {
                    HostIdentityReasonCode::PathInvalid
                } else {
                    HostIdentityReasonCode::PeerExited
                };
                return Err(self.rejection(
                    socket_path,
                    Some(peer_pid),
                    Some(observed_link),
                    None,
                    reason,
                ));
            }
        };
        let Some(observed_slot) = observed_executable.parent().map(Path::to_path_buf) else {
            return Err(self.rejection(
                socket_path,
                Some(peer_pid),
                Some(observed_executable),
                None,
                HostIdentityReasonCode::IdentityAmbiguous,
            ));
        };

        if observed_executable != self.expected_executable {
            return Err(self.rejection(
                socket_path,
                Some(peer_pid),
                Some(observed_executable),
                Some(observed_slot),
                HostIdentityReasonCode::SlotMismatch,
            ));
        }

        let diagnostic = HostIdentityDiagnostic {
            socket_path: socket_path.to_path_buf(),
            peer_pid: Some(peer_pid),
            observed_executable: Some(observed_executable),
            expected_slot: self.expected_slot.clone(),
            observed_slot: Some(observed_slot),
            guard_result: "ACCEPTED",
            reason_code: HostIdentityReasonCode::Match,
        };
        tracing::info!(
            socket_path = %diagnostic.socket_path.display(),
            peer_pid = diagnostic.peer_pid,
            observed_executable = %diagnostic.observed_executable.as_deref().unwrap().display(),
            expected_slot = %diagnostic.expected_slot.display(),
            observed_slot = %diagnostic.observed_slot.as_deref().unwrap().display(),
            guard_result = diagnostic.guard_result,
            reason_code = %diagnostic.reason_code,
            "ACP host identity guard"
        );
        Ok(())
    }

    fn rejection(
        &self,
        socket_path: &Path,
        peer_pid: Option<u32>,
        observed_executable: Option<PathBuf>,
        observed_slot: Option<PathBuf>,
        reason_code: HostIdentityReasonCode,
    ) -> HostIdentityError {
        let diagnostic = HostIdentityDiagnostic {
            socket_path: socket_path.to_path_buf(),
            peer_pid,
            observed_executable,
            expected_slot: self.expected_slot.clone(),
            observed_slot,
            guard_result: "REJECTED",
            reason_code,
        };
        if let Ok(mut rejection) = self.last_rejection.lock() {
            *rejection = Some(diagnostic.clone());
        }
        tracing::warn!(
            socket_path = %diagnostic.socket_path.display(),
            peer_pid = diagnostic.peer_pid,
            observed_executable = ?diagnostic.observed_executable,
            expected_slot = %diagnostic.expected_slot.display(),
            observed_slot = ?diagnostic.observed_slot,
            guard_result = diagnostic.guard_result,
            reason_code = %diagnostic.reason_code,
            "ACP host identity guard"
        );
        HostIdentityError {
            diagnostic: Box::new(diagnostic),
        }
    }

    fn take_rejection(&self) -> Option<HostIdentityDiagnostic> {
        self.last_rejection
            .lock()
            .ok()
            .and_then(|mut diagnostic| diagnostic.take())
    }

    #[cfg(test)]
    pub(crate) fn verify_observed_executable_for_test(
        &self,
        peer_pid: u32,
        observed_executable: &Path,
    ) -> Result<(), HostIdentityError> {
        let observed_executable = fs::canonicalize(observed_executable).map_err(|_| {
            self.rejection(
                Path::new("test.sock"),
                Some(peer_pid),
                Some(observed_executable.to_path_buf()),
                None,
                HostIdentityReasonCode::PathInvalid,
            )
        })?;
        let observed_slot = observed_executable.parent().map(Path::to_path_buf);
        if observed_executable == self.expected_executable {
            return Ok(());
        }
        Err(self.rejection(
            Path::new("test.sock"),
            Some(peer_pid),
            Some(observed_executable),
            observed_slot,
            HostIdentityReasonCode::SlotMismatch,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpTransportMode {
    Mpsc,
    Local,
    Auto,
}

impl AcpTransportMode {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::parse(&std::env::var("NEXUM_ACP_TRANSPORT").unwrap_or_else(|_| "auto".to_string()))
    }

    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "mpsc" => Ok(Self::Mpsc),
            "local" => Ok(Self::Local),
            value => {
                anyhow::bail!("NEXUM_ACP_TRANSPORT must be mpsc, local, or auto; got {value:?}")
            }
        }
    }
}

/// Connects a local host according to the selected policy. `local` only accepts
/// an already-running external host. `auto` is the only mode allowed to spawn.
pub async fn connect_local(mode: AcpTransportMode) -> anyhow::Result<LocalAcpTransport> {
    // Resolve and validate the runtime before deriving a socket path or
    // inspecting any peer/process. An invalid XDG runtime is terminal and
    // cannot reach auto-start or stale-host recovery.
    let socket = default_local_socket_path()?;
    let guard = HostIdentityGuard::for_current_runtime()?;
    connect_local_at_with_guard(mode, &socket, &guard).await
}

pub(crate) async fn connect_local_at_with_guard(
    mode: AcpTransportMode,
    socket: &Path,
    guard: &HostIdentityGuard,
) -> anyhow::Result<LocalAcpTransport> {
    match mode {
        AcpTransportMode::Local => connect_ready_guarded(socket, READY_TIMEOUT, guard)
            .await
            .map_err(|error| {
                let context = format!(
                    "local ACP host is required and must be ready at {}: {error}",
                    socket.display()
                );
                anyhow::Error::new(error).context(context)
            }),
        AcpTransportMode::Auto => {
            let host = guard.expected_executable().to_path_buf();
            ensure_auto_host(socket, READY_TIMEOUT, guard, move || {
                spawn_local_host_at(&host)
            })
            .await
        }
        AcpTransportMode::Mpsc => anyhow::bail!("MPSC does not use a local ACP host"),
    }
}

/// Bridges the selected local transport into the object-safe client boundary.
pub async fn connect_local_client(
    mode: AcpTransportMode,
) -> anyhow::Result<std::sync::Arc<dyn AcpClientTransport>> {
    match connect_local(mode).await {
        Ok(transport) => Ok(std::sync::Arc::new(transport)),
        Err(error) => {
            if let Some(message) = host_identity_rejection_message(&error)
                .or_else(|| runtime_directory_rejection_message(&error))
            {
                Ok(std::sync::Arc::new(RejectedHostTransport { message }))
            } else {
                Err(error)
            }
        }
    }
}

pub(crate) fn host_identity_rejection_message(error: &anyhow::Error) -> Option<String> {
    error.chain().find_map(|cause| {
        let transport_error = cause.downcast_ref::<LocalTransportError>()?;
        match transport_error {
            LocalTransportError::HostIdentity(message) => Some(message.clone()),
            _ => None,
        }
    })
}

pub(crate) fn runtime_directory_rejection_message(error: &anyhow::Error) -> Option<String> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<RuntimeDirectoryError>())
        .map(ToString::to_string)
}

/// A rejected peer never regains access to the socket. This facade exists only
/// so the already-established TUI terminal path can surface the structured
/// guard error and restore an in-flight prompt exactly once.
struct RejectedHostTransport {
    message: String,
}

#[async_trait::async_trait]
impl AcpTransport for RejectedHostTransport {
    async fn send_request(
        &self,
        _method: &str,
        _params: serde_json::Value,
    ) -> Result<serde_json::Value, AcpError> {
        Err(AcpError::new(-32090, self.message.clone()))
    }

    async fn send_notification(
        &self,
        _method: &str,
        _params: serde_json::Value,
    ) -> Result<(), AcpError> {
        Err(AcpError::new(-32090, self.message.clone()))
    }

    async fn recv(&self) -> Option<IncomingMessage> {
        std::future::pending().await
    }

    async fn send_response(
        &self,
        _id: RequestId,
        _result: Result<serde_json::Value, AcpError>,
    ) -> Result<(), AcpError> {
        Err(AcpError::new(-32090, self.message.clone()))
    }

    async fn close(&self) -> Result<(), AcpError> {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn rejected_host_transport_for_test(
    message: impl Into<String>,
) -> std::sync::Arc<dyn AcpClientTransport> {
    std::sync::Arc::new(RejectedHostTransport {
        message: message.into(),
    })
}

fn spawn_local_host_at(host: &Path) -> anyhow::Result<()> {
    let state_root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local").join("state"))
        })
        .ok_or_else(|| anyhow::anyhow!("XDG_STATE_HOME/HOME unavailable for ACP diagnostics"))?
        .join("nexum");
    fs::create_dir_all(&state_root)?;
    fs::set_permissions(
        &state_root,
        <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
    )?;
    let stderr_path = state_root.join("acp-host.stderr.log");
    let diagnostic_path = state_root.join("acp-host-exit.json");
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&stderr_path)?;

    let mut cmd = std::process::Command::new(host);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .arg("--diagnostic")
        .arg(&diagnostic_path);
    // The auto-started host is owned by this TUI. PR_SET_PDEATHSIG closes it
    // gracefully if the parent exits, preventing a reparented stale singleton.
    // Linux-only: macOS and Windows do not provide prctl(2); there the child
    // inherits the parent's death via normal reparenting semantics.
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
        .map(|mut child| {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        })
        // Contexto explícito: sin esto el fallo era un "os error 2" pelado.
        // El caso típico es el binario `nexum-acp-host` ausente junto al
        // `nexum` instalado (debe viajar en el artefacto como sibling).
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to spawn the ACP host binary at {}: {e} \
                 (expected `nexum-acp-host` next to the nexum executable or in PATH)",
                host.display()
            )
        })
}

pub(crate) fn host_binary_path_from_executable(executable: &Path) -> PathBuf {
    let sibling = executable
        .parent()
        .map(|directory| directory.join("nexum-acp-host"));
    sibling
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("nexum-acp-host"))
}

struct StartupLock(PathBuf);

impl Drop for StartupLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn try_startup_lock(socket: &Path) -> anyhow::Result<Option<StartupLock>> {
    let parent = socket
        .parent()
        .ok_or_else(|| io::Error::other("local ACP socket has no parent"))?;
    ensure_private_runtime_directory(parent)?;
    let lock = parent.join("acp.startup.lock");
    match OpenOptions::new().write(true).create_new(true).open(&lock) {
        Ok(_) => Ok(Some(StartupLock(lock))),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Coordinates a single auto-start attempt. A caller that did not acquire the
/// lock only waits for the winner's healthy handshake and never starts a second
/// host. Only an absent/refused socket is eligible for a startup attempt; a
/// visible host that is incompatible never falls back to another runtime.
pub(crate) async fn ensure_auto_host<F>(
    socket: &Path,
    ready_timeout: Duration,
    guard: &HostIdentityGuard,
    spawn: F,
) -> anyhow::Result<LocalAcpTransport>
where
    F: FnOnce() -> anyhow::Result<()>,
{
    let mut last_error = match connect_ready_guarded(socket, RETRY_INTERVAL, guard).await {
        Ok(transport) => return Ok(transport),
        Err(error)
            if error.is_protocol_failure() && !rejection_is_recoverable_slot_mismatch(guard) =>
        {
            return Err(error.into())
        }
        Err(error) => error,
    };

    // Keep the lock until readiness is observed so a second TUI cannot launch
    // a duplicate host while the first child is still binding its socket.
    let startup_lock =
        if last_error.permits_auto_start() || rejection_is_recoverable_slot_mismatch(guard) {
            try_startup_lock(socket)?
        } else {
            None
        };
    if startup_lock.is_some() {
        match connect_ready_guarded(socket, RETRY_INTERVAL, guard).await {
            Ok(transport) => return Ok(transport),
            Err(error)
                if error.is_protocol_failure() && rejection_is_recoverable_slot_mismatch(guard) =>
            {
                recover_rejected_stale_host(socket, guard).await?;
                spawn()?;
                last_error = error;
            }
            Err(error) if error.is_protocol_failure() => return Err(error.into()),
            Err(error) if error.permits_auto_start() => {
                if prepare_unavailable_socket(socket)? {
                    spawn()?;
                }
                last_error = error;
            }
            Err(error) => last_error = error,
        }
    }

    let deadline = Instant::now() + ready_timeout;
    while Instant::now() < deadline {
        match connect_ready_guarded(socket, RETRY_INTERVAL, guard).await {
            Ok(transport) => return Ok(transport),
            Err(error)
                if error.is_protocol_failure()
                    && startup_lock.is_none()
                    && rejection_is_recoverable_slot_mismatch(guard) =>
            {
                // Another TUI owns the startup lock and is replacing the stale
                // peer. This caller waits for that single winner.
                last_error = error;
            }
            Err(error) if error.is_protocol_failure() => return Err(error.into()),
            Err(error) => last_error = error,
        }
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
    Err(anyhow::anyhow!(
        "local ACP host did not become ready: {}",
        last_error
    ))
}

fn rejection_is_recoverable_slot_mismatch(guard: &HostIdentityGuard) -> bool {
    guard
        .last_rejection
        .lock()
        .ok()
        .and_then(|diagnostic| diagnostic.clone())
        .is_some_and(|diagnostic| {
            diagnostic.reason_code == HostIdentityReasonCode::SlotMismatch
                && diagnostic.peer_pid.is_some()
        })
}

async fn recover_rejected_stale_host(
    socket: &Path,
    guard: &HostIdentityGuard,
) -> anyhow::Result<()> {
    let diagnostic = guard
        .take_rejection()
        .ok_or_else(|| anyhow::anyhow!("HOST_IDENTITY_AMBIGUOUS: missing rejected peer"))?;
    if diagnostic.reason_code != HostIdentityReasonCode::SlotMismatch {
        anyhow::bail!("{} is not eligible for recovery", diagnostic.reason_code);
    }
    let pid = diagnostic
        .peer_pid
        .ok_or_else(|| anyhow::anyhow!("HOST_PEER_PID_UNAVAILABLE"))?;
    let observed = diagnostic
        .observed_executable
        .ok_or_else(|| anyhow::anyhow!("HOST_EXECUTABLE_UNRESOLVED"))?;
    if observed.file_name().and_then(|name| name.to_str()) != Some("nexum-acp-host") {
        anyhow::bail!("HOST_IDENTITY_AMBIGUOUS: rejected peer is not nexum-acp-host");
    }

    let current_observed = fs::canonicalize(format!("/proc/{pid}/exe"))
        .map_err(|_| anyhow::anyhow!("HOST_PEER_EXITED before graceful shutdown"))?;
    if current_observed != observed {
        anyhow::bail!("HOST_IDENTITY_AMBIGUOUS: peer executable changed before shutdown");
    }
    let owner_uid = fs::metadata(format!("/proc/{pid}"))?.uid();
    if owner_uid != unsafe { libc::geteuid() } {
        anyhow::bail!("PERMISSION_ERROR: rejected ACP host belongs to another uid");
    }
    let signal_result = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if signal_result != 0 {
        anyhow::bail!(
            "failed to terminate rejected ACP host gracefully: {}",
            io::Error::last_os_error()
        );
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_running(pid) && Instant::now() < deadline {
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
    if process_is_running(pid) {
        anyhow::bail!("stale ACP host did not exit gracefully after SIGTERM");
    }

    // The old host owns cleanup. Removal here is only allowed after its exact
    // PID is gone and the path is demonstrably refused (dead-owner residue).
    if socket.exists() {
        match std::os::unix::net::UnixStream::connect(socket) {
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                fs::remove_file(socket)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => anyhow::bail!("a new ACP host rebound the socket during recovery"),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn process_is_running(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // /proc/<pid>/stat: the state is the first token after the parenthesized
    // command name. A zombie owns no file descriptors and therefore no socket.
    stat.rsplit_once(')')
        .and_then(|(_, tail)| tail.split_whitespace().next())
        != Some("Z")
}

async fn connect_ready_guarded(
    socket: &Path,
    budget: Duration,
    guard: &HostIdentityGuard,
) -> Result<LocalAcpTransport, nexum_acp::transport::local::LocalTransportError> {
    LocalAcpTransport::connect_ready_guarded(socket, budget, |stream| {
        guard
            .verify(socket, stream)
            .map_err(|error| error.to_string())
    })
    .await
}

fn prepare_unavailable_socket(socket: &Path) -> io::Result<bool> {
    match std::os::unix::net::UnixStream::connect(socket) {
        Ok(_) => Ok(false),
        Err(error) if socket.exists() && error.kind() == io::ErrorKind::ConnectionRefused => {
            fs::remove_file(socket)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}
