use std::{
    io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

use nexum_acp::transport::{types::IncomingMessage, socket::SocketTransport, AcpTransport};
use serde_json::json;
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::tokio::Stream as LocalSocketStream;

use super::bootstrap::{
    connect_local, connect_local_at_with_guard, ensure_auto_host, host_binary_path_from_executable,
    host_identity_rejection_message, rejected_host_transport_for_test,
    runtime_directory_rejection_message, AcpTransportMode, HostIdentityGuard,
    HostIdentityReasonCode, HostIdentitySource,
};

fn runtime_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_runtime_environment<T>(runtime: &Path, guard: &Path, action: impl FnOnce() -> T) -> T {
    let _lock = runtime_env_lock().lock().unwrap();
    let original_runtime = std::env::var_os("XDG_RUNTIME_DIR");
    let original_guard = std::env::var_os("NEXUM_RUNTIME_ROOT_GUARD");
    std::env::set_var("XDG_RUNTIME_DIR", runtime);
    std::env::set_var("NEXUM_RUNTIME_ROOT_GUARD", guard);
    let result = action();
    match original_runtime {
        Some(value) => std::env::set_var("XDG_RUNTIME_DIR", value),
        None => std::env::remove_var("XDG_RUNTIME_DIR"),
    }
    match original_guard {
        Some(value) => std::env::set_var("NEXUM_RUNTIME_ROOT_GUARD", value),
        None => std::env::remove_var("NEXUM_RUNTIME_ROOT_GUARD"),
    }
    result
}

#[cfg(not(unix))]
#[derive(Default)]
struct NeverSignal;

#[cfg(not(unix))]
impl NeverSignal {
    fn recv(&mut self) -> futures_util::future::Pending<()> {
        futures_util::future::pending()
    }
}

fn secure_directory(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

async fn bind_server(socket: &std::path::Path) -> io::Result<LocalSocketListener> {
    let name = nexum_acp::transport::local::local_socket_name(socket)?;
    interprocess::local_socket::ListenerOptions::new().name(name).create_tokio()
}

fn current_test_process_guard() -> HostIdentityGuard {
    HostIdentityGuard::for_expected_host(&std::env::current_exe().unwrap()).unwrap()
}

async fn serve_health(listener: LocalSocketListener, connections: usize) {
    for _ in 0..connections {
        let stream = listener.accept().await.unwrap();
        let transport = SocketTransport::from_stream(stream);
        if let Some(IncomingMessage::Request { id, method, .. }) = transport.recv().await {
            assert_eq!(method, "health");
            transport
                .send_response(
                    id,
                    Ok(json!({
                        "protocol_version": nexum_acp::transport::socket::LOCAL_PROTOCOL_VERSION,
                        "runtime_available": true,
                        "health": "ready"
                    })),
                )
                .await
                .unwrap();
        }
    }
}

#[test]
fn test_transport_mode_defaults_to_auto_and_mpsc_is_explicit_only() {
    assert_eq!(AcpTransportMode::parse("").unwrap(), AcpTransportMode::Auto);
    assert_eq!(
        AcpTransportMode::parse("MPSC").unwrap(),
        AcpTransportMode::Mpsc
    );
    assert_eq!(
        AcpTransportMode::parse("local").unwrap(),
        AcpTransportMode::Local
    );
    assert_eq!(
        AcpTransportMode::parse("auto").unwrap(),
        AcpTransportMode::Auto
    );
    assert!(AcpTransportMode::parse("other").is_err());
}

#[test]
fn invalid_runtime_does_not_connect_socket() {
    let guard = tempfile::tempdir().unwrap();
    secure_directory(guard.path());
    let missing = guard.path().join("missing-runtime");
    let error = with_runtime_environment(&missing, guard.path(), || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(connect_local(AcpTransportMode::Local))
            .unwrap_err()
    });
    assert!(error.to_string().contains("RUNTIME_DIRECTORY_NOT_FOUND"));
    assert!(!missing.join("nexum/acp.sock").exists());
}

#[test]
fn invalid_runtime_does_not_spawn_host() {
    let guard = tempfile::tempdir().unwrap();
    secure_directory(guard.path());
    let missing = guard.path().join("missing-runtime");
    let error = with_runtime_environment(&missing, guard.path(), || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(connect_local(AcpTransportMode::Auto))
            .unwrap_err()
    });
    assert!(error.to_string().contains("RUNTIME_DIRECTORY_NOT_FOUND"));
    assert!(!missing.exists());
}

#[test]
fn invalid_runtime_does_not_signal_pid() {
    let guard = tempfile::tempdir().unwrap();
    secure_directory(guard.path());
    let relative = PathBuf::from("relative-runtime");
    let error = with_runtime_environment(&relative, guard.path(), || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(connect_local(AcpTransportMode::Auto))
            .unwrap_err()
    });
    assert!(error.to_string().contains("RUNTIME_DIRECTORY_NOT_ABSOLUTE"));
}

#[test]
fn invalid_runtime_does_not_unlink_socket() {
    let guard = tempfile::tempdir().unwrap();
    secure_directory(guard.path());
    let runtime = guard.path().join("unsafe-runtime");
    std::fs::create_dir(&runtime).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let error = with_runtime_environment(&runtime, guard.path(), || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(connect_local(AcpTransportMode::Auto))
            .unwrap_err()
    });
    assert!(error
        .to_string()
        .contains("RUNTIME_DIRECTORY_UNSAFE_PERMISSIONS"));
    assert!(!runtime.join("nexum/acp.sock").exists());
}

#[test]
fn scope_violation_does_not_touch_real_runtime() {
    let guard = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    secure_directory(guard.path());
    secure_directory(outside.path());
    let error = with_runtime_environment(outside.path(), guard.path(), || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(connect_local(AcpTransportMode::Auto))
            .unwrap_err()
    });
    assert!(error.to_string().contains("RUNTIME_SCOPE_VIOLATION"));
    assert!(!outside.path().join("nexum/acp.sock").exists());
}

#[test]
fn isolated_runtime_never_uses_real_socket() {
    let guard = tempfile::tempdir().unwrap();
    let runtime = guard.path().join("runtime");
    secure_directory(guard.path());
    std::fs::create_dir(&runtime).unwrap();
    secure_directory(&runtime);
    let socket = with_runtime_environment(&runtime, guard.path(), || {
        nexum_acp::transport::local::default_local_socket_path().unwrap()
    });
    assert!(socket.starts_with(guard.path()));
    assert_ne!(socket, PathBuf::from("/run/user/1000/nexum/acp.sock"));
}

#[test]
fn isolated_runtime_never_uses_real_process() {
    let guard = tempfile::tempdir().unwrap();
    let runtime = guard.path().join("runtime");
    secure_directory(guard.path());
    std::fs::create_dir(&runtime).unwrap();
    secure_directory(&runtime);
    let socket = with_runtime_environment(&runtime, guard.path(), || {
        nexum_acp::transport::local::default_local_socket_path().unwrap()
    });
    assert!(socket.parent().unwrap().starts_with(guard.path()));
}

#[test]
fn test_auto_resolves_host_binary_next_to_tui_executable() {
    let temp = tempfile::TempDir::new().unwrap();
    let tui = temp.path().join("nexum");
    let host = temp.path().join("nexum-acp-host");
    std::fs::write(&tui, "").unwrap();
    std::fs::write(&host, "").unwrap();

    assert_eq!(host_binary_path_from_executable(&tui), host);
}

#[test]
fn test_host_from_expected_slot_is_accepted() {
    let temp = tempfile::TempDir::new().unwrap();
    let slot = temp.path().join("slot-A");
    std::fs::create_dir(&slot).unwrap();
    let host = slot.join("nexum-acp-host");
    std::fs::write(&host, "").unwrap();

    let guard = HostIdentityGuard::for_expected_host(&host).unwrap();

    assert!(guard.verify_observed_executable_for_test(7, &host).is_ok());
}

#[test]
fn stale_host_is_rejected() {
    let temp = tempfile::TempDir::new().unwrap();
    let slot_a = temp.path().join("slot-A");
    let slot_b = temp.path().join("slot-B");
    std::fs::create_dir(&slot_a).unwrap();
    std::fs::create_dir(&slot_b).unwrap();
    let expected = slot_a.join("nexum-acp-host");
    let observed = slot_b.join("nexum-acp-host");
    std::fs::write(&expected, "A").unwrap();
    std::fs::write(&observed, "B").unwrap();

    let error = HostIdentityGuard::for_expected_host(&expected)
        .unwrap()
        .verify_observed_executable_for_test(8, &observed)
        .unwrap_err();

    assert_eq!(error.reason_code(), HostIdentityReasonCode::SlotMismatch);
    assert_eq!(
        error.diagnostic().observed_slot.as_deref(),
        Some(slot_b.as_path())
    );
}

#[cfg(unix)]
#[test]
fn test_current_symlink_is_canonicalized_to_the_expected_slot() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::TempDir::new().unwrap();
    let slot = temp.path().join("slot-A");
    std::fs::create_dir(&slot).unwrap();
    let host = slot.join("nexum-acp-host");
    std::fs::write(&host, "A").unwrap();
    let current = temp.path().join("current");
    symlink(&slot, &current).unwrap();

    let guard = HostIdentityGuard::for_expected_host(&current.join("nexum-acp-host")).unwrap();

    assert_eq!(guard.expected_executable(), host.as_path());
    assert!(guard.verify_observed_executable_for_test(9, &host).is_ok());
}

#[cfg(unix)]
#[test]
fn test_expected_host_symlink_cannot_escape_the_expected_slot() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::TempDir::new().unwrap();
    let slot = temp.path().join("slot-A");
    let outside = temp.path().join("outside");
    std::fs::create_dir(&slot).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let outside_host = outside.join("nexum-acp-host");
    std::fs::write(&outside_host, "outside").unwrap();
    symlink(&outside_host, slot.join("nexum-acp-host")).unwrap();

    let error = HostIdentityGuard::for_expected_host(&slot.join("nexum-acp-host")).unwrap_err();

    assert!(error.to_string().contains("HOST_SLOT_MISMATCH"));
}

#[test]
fn test_deceptive_slot_prefix_is_rejected_by_canonical_path_equality() {
    let temp = tempfile::TempDir::new().unwrap();
    let slot = temp.path().join("slot-A");
    let malicious_slot = temp.path().join("slot-A-malicioso");
    std::fs::create_dir(&slot).unwrap();
    std::fs::create_dir(&malicious_slot).unwrap();
    let expected = slot.join("nexum-acp-host");
    let observed = malicious_slot.join("nexum-acp-host");
    std::fs::write(&expected, "A").unwrap();
    std::fs::write(&observed, "B").unwrap();

    let error = HostIdentityGuard::for_expected_host(&expected)
        .unwrap()
        .verify_observed_executable_for_test(10, &observed)
        .unwrap_err();

    assert_eq!(error.reason_code(), HostIdentityReasonCode::SlotMismatch);
}

struct FakeIdentitySource {
    pid: io::Result<Option<u32>>,
    executable: io::Result<PathBuf>,
    peer_exists: bool,
    canonicalize_fails: bool,
}

impl HostIdentitySource for FakeIdentitySource {
    fn peer_pid(&self, _stream: &LocalSocketStream) -> io::Result<Option<u32>> {
        self.pid
            .as_ref()
            .map(|pid| *pid)
            .map_err(|error| io::Error::new(error.kind(), "fake peer PID error"))
    }

    fn executable_for_pid(&self, _pid: u32) -> io::Result<PathBuf> {
        self.executable
            .as_ref()
            .cloned()
            .map_err(|error| io::Error::new(error.kind(), "fake proc error"))
    }

    fn peer_exists(&self, _pid: u32) -> bool {
        self.peer_exists
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        if self.canonicalize_fails {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fake canonicalization error",
            ))
        } else {
            std::fs::canonicalize(path)
        }
    }
}

fn guard_fixture() -> (tempfile::TempDir, HostIdentityGuard) {
    let temp = tempfile::TempDir::new().unwrap();
    let slot = temp.path().join("slot-A");
    std::fs::create_dir(&slot).unwrap();
    let host = slot.join("nexum-acp-host");
    std::fs::write(&host, "A").unwrap();
    let guard = HostIdentityGuard::for_expected_host(&host).unwrap();
    (temp, guard)
}
async fn test_stream_pair() -> (LocalSocketStream, LocalSocketStream) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("guard.sock");
    let name = nexum_acp::transport::local::local_socket_name(&path).unwrap();
    let listener = interprocess::local_socket::ListenerOptions::new()
        .name(name)
        .create_tokio()
        .unwrap();
    let client_task = tokio::spawn(async move {
        let name = nexum_acp::transport::local::local_socket_name(&path).unwrap();
        LocalSocketStream::connect(name).await.unwrap()
    });
    let server = listener.accept().await.unwrap();
    (client_task.await.unwrap(), server)
}


#[tokio::test]
async fn test_peer_pid_unavailable_fails_closed() {
    let (_temp, guard) = guard_fixture();
    let (stream, _peer) = test_stream_pair().await;
    let source = FakeIdentitySource {
        pid: Ok(None),
        executable: Err(io::Error::new(io::ErrorKind::NotFound, "unused")),
        peer_exists: true,
        canonicalize_fails: false,
    };

    let error = guard
        .verify_with_source(Path::new("isolated.sock"), &stream, &source)
        .unwrap_err();

    assert_eq!(
        error.reason_code(),
        HostIdentityReasonCode::PeerPidUnavailable
    );
}

#[tokio::test]
async fn test_proc_executable_unresolvable_fails_closed() {
    let (_temp, guard) = guard_fixture();
    let (stream, _peer) = test_stream_pair().await;
    let source = FakeIdentitySource {
        pid: Ok(Some(44)),
        executable: Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
        peer_exists: true,
        canonicalize_fails: false,
    };

    let error = guard
        .verify_with_source(Path::new("isolated.sock"), &stream, &source)
        .unwrap_err();

    assert_eq!(
        error.reason_code(),
        HostIdentityReasonCode::ExecutableUnresolved
    );
}

#[tokio::test]
async fn test_peer_exit_during_verification_fails_closed() {
    let (_temp, guard) = guard_fixture();
    let (stream, _peer) = test_stream_pair().await;
    let source = FakeIdentitySource {
        pid: Ok(Some(45)),
        executable: Err(io::Error::new(io::ErrorKind::NotFound, "gone")),
        peer_exists: false,
        canonicalize_fails: false,
    };

    let error = guard
        .verify_with_source(Path::new("isolated.sock"), &stream, &source)
        .unwrap_err();

    assert_eq!(error.reason_code(), HostIdentityReasonCode::PeerExited);
}

#[tokio::test]
async fn test_invalid_observed_path_fails_closed() {
    let (_temp, guard) = guard_fixture();
    let (stream, _peer) = test_stream_pair().await;
    let source = FakeIdentitySource {
        pid: Ok(Some(46)),
        executable: Ok(PathBuf::from("/proc/46/exe")),
        peer_exists: true,
        canonicalize_fails: true,
    };

    let error = guard
        .verify_with_source(Path::new("isolated.sock"), &stream, &source)
        .unwrap_err();

    assert_eq!(error.reason_code(), HostIdentityReasonCode::PathInvalid);
}

#[test]
fn test_sanitized_diagnostic_contains_only_identity_fields() {
    let temp = tempfile::TempDir::new().unwrap();
    let expected_slot = temp.path().join("slot-A");
    let observed_slot = temp.path().join("slot-B");
    std::fs::create_dir(&expected_slot).unwrap();
    std::fs::create_dir(&observed_slot).unwrap();
    let expected = expected_slot.join("nexum-acp-host");
    let observed = observed_slot.join("nexum-acp-host");
    std::fs::write(&expected, "A").unwrap();
    std::fs::write(&observed, "B").unwrap();
    let diagnostic = HostIdentityGuard::for_expected_host(&expected)
        .unwrap()
        .verify_observed_executable_for_test(47, &observed)
        .unwrap_err()
        .to_string();

    for field in [
        "socket_path=",
        "peer_pid=",
        "observed_executable=",
        "expected_slot=",
        "observed_slot=",
        "guard_result=",
        "reason_code=",
    ] {
        assert!(diagnostic.contains(field));
    }
    for secret in [
        "prompt-ultra-secreto",
        "token-ultra-secreto",
        "cookie-ultra-secreta",
        "credential-ultra-secreta",
    ] {
        assert!(!diagnostic.contains(secret));
    }
}

#[test]
fn test_structured_identity_error_survives_bootstrap_context() {
    let error = anyhow::Error::new(
        nexum_acp::transport::local::LocalTransportError::HostIdentity(
            "reason_code=HOST_SLOT_MISMATCH".into(),
        ),
    )
    .context("local ACP host bootstrap");

    assert_eq!(
        host_identity_rejection_message(&error).as_deref(),
        Some("reason_code=HOST_SLOT_MISMATCH")
    );
}

#[test]
fn invalid_runtime_reaches_tui_as_structured_terminal_error() {
    let error = anyhow::Error::new(
        nexum_acp::transport::local::RuntimeDirectoryError::NotFound {
            path: PathBuf::from("/isolated/missing"),
        },
    )
    .context("local ACP runtime bootstrap");

    assert_eq!(
        runtime_directory_rejection_message(&error).as_deref(),
        Some("RUNTIME_DIRECTORY_NOT_FOUND: /isolated/missing")
    );
}

#[tokio::test]
async fn test_local_requires_a_visible_ready_host() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("missing.sock");

    let guard = current_test_process_guard();
    let error = connect_local_at_with_guard(AcpTransportMode::Local, &socket, &guard)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("local ACP host is required"));
    assert!(error.to_string().contains(&socket.display().to_string()));
}

#[tokio::test]
async fn acp_host_healthcheck_passes() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await.unwrap();
    tokio::spawn(serve_health(listener, 1));
    let guard = current_test_process_guard();

    let transport = ensure_auto_host(&socket, Duration::from_secs(1), &guard, || {
        panic!("a live host must not be spawned twice")
    })
    .await
    .unwrap();

    assert_eq!(transport.socket_path(), socket.as_path());
}

#[tokio::test]
async fn test_auto_rejects_a_visible_protocol_mismatch_without_spawning() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await.unwrap();
    tokio::spawn(async move {
        let stream = listener.accept().await.unwrap();
        let transport = SocketTransport::from_stream(stream);
        let Some(IncomingMessage::Request { id, method, .. }) = transport.recv().await else {
            panic!("expected health request");
        };
        assert_eq!(method, "health");
        transport
            .send_response(
                id,
                Ok(json!({
                    "protocol_version": 999,
                    "runtime_available": true,
                    "health": "ready"
                })),
            )
            .await
            .unwrap();
    });
    let spawns = Arc::new(AtomicUsize::new(0));
    let spawn_count = spawns.clone();
    let guard = current_test_process_guard();

    let error = ensure_auto_host(&socket, Duration::from_secs(1), &guard, move || {
        spawn_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .await
    .unwrap_err();

    assert!(error.to_string().contains("protocol"));
    assert_eq!(spawns.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_auto_recovers_stale_socket_before_one_spawn() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let stale = bind_server(&socket).await.unwrap();
    drop(stale);
    let socket_for_host = socket.clone();
    let spawns = Arc::new(AtomicUsize::new(0));
    let spawn_count = spawns.clone();
    let guard = current_test_process_guard();

    let transport = ensure_auto_host(&socket, Duration::from_secs(1), &guard, move || {
        spawn_count.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(async move {
            let listener = bind_server(&socket_for_host).await.unwrap();
            serve_health(listener, 1).await;
        });
        Ok(())
    })
    .await
    .unwrap();

    assert_eq!(transport.socket_path(), socket.as_path());
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_auto_surfaces_host_startup_error() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let guard = current_test_process_guard();

    let error = ensure_auto_host(&socket, Duration::from_secs(1), &guard, || {
        anyhow::bail!("unable to start isolated ACP host")
    })
    .await
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("unable to start isolated ACP host"));
}

#[tokio::test]
async fn test_auto_concurrent_callers_spawn_exactly_one_mock_host() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let spawns = Arc::new(AtomicUsize::new(0));
    let first_socket = socket.clone();
    let first_spawns = spawns.clone();
    let second_socket = socket.clone();
    let second_spawns = spawns.clone();
    let guard = current_test_process_guard();

    let (first, second) = tokio::join!(
        ensure_auto_host(&socket, Duration::from_secs(1), &guard, move || {
            first_spawns.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let listener = bind_server(&first_socket).await.unwrap();
                serve_health(listener, 2).await;
            });
            Ok(())
        }),
        ensure_auto_host(&socket, Duration::from_secs(1), &guard, move || {
            second_spawns.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let listener = bind_server(&second_socket).await.unwrap();
                serve_health(listener, 2).await;
            });
            Ok(())
        })
    );

    assert!(first.is_ok());
    assert!(second.is_ok());
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_two_local_clients_share_host_identity_capabilities_provider_and_model() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await.unwrap();
    let server = tokio::spawn(serve_shared_host(listener, 2));
    let guard = current_test_process_guard();

    let first = connect_local_at_with_guard(AcpTransportMode::Local, &socket, &guard)
        .await
        .unwrap();
    let second = connect_local_at_with_guard(AcpTransportMode::Local, &socket, &guard)
        .await
        .unwrap();
    let first_identity = first
        .send_request("runtime/identity", json!({}))
        .await
        .unwrap();
    let second_identity = second
        .send_request("runtime/identity", json!({}))
        .await
        .unwrap();
    let first_capabilities = first
        .send_request("runtime/capabilities", json!({}))
        .await
        .unwrap();
    let second_capabilities = second
        .send_request("runtime/capabilities", json!({}))
        .await
        .unwrap();

    assert_eq!(
        first_identity["runtime_instance_id"],
        second_identity["runtime_instance_id"]
    );
    assert_eq!(first_identity["provider"], "openai");
    assert_eq!(first_identity["model"], "mock-host-model");
    assert_eq!(first_identity["provider"], second_identity["provider"]);
    assert_eq!(first_identity["model"], second_identity["model"]);
    assert_eq!(
        first_capabilities["capabilities"]["hash"],
        second_capabilities["capabilities"]["hash"]
    );

    drop(first);
    drop(second);
    server.await.unwrap();
}

#[tokio::test]
async fn test_closing_a_tui_local_client_does_not_stop_the_host_listener() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await.unwrap();
    let server = tokio::spawn(serve_health(listener, 2));
    let guard = current_test_process_guard();

    let first = connect_local_at_with_guard(AcpTransportMode::Local, &socket, &guard)
        .await
        .unwrap();
    first.close().await.unwrap();
    drop(first);

    let second = connect_local_at_with_guard(AcpTransportMode::Local, &socket, &guard)
        .await
        .unwrap();
    assert_eq!(second.socket_path(), socket.as_path());
    drop(second);
    server.await.unwrap();
}

async fn assert_slot_mismatch_sends_zero_rpc() {
    let temp = tempfile::TempDir::new().unwrap();
    let expected_slot = temp.path().join("slot-A");
    std::fs::create_dir(&expected_slot).unwrap();
    let expected_host = expected_slot.join("nexum-acp-host");
    std::fs::write(&expected_host, "not-the-running-test-process").unwrap();
    let guard = HostIdentityGuard::for_expected_host(&expected_host).unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await.unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let server_requests = requests.clone();
    let server = tokio::spawn(async move {
        let stream = listener.accept().await.unwrap();
        let transport = SocketTransport::from_stream(stream);
        while let Some(IncomingMessage::Request { .. }) = transport.recv().await {
            server_requests.fetch_add(1, Ordering::SeqCst);
        }
    });

    let error = connect_local_at_with_guard(AcpTransportMode::Local, &socket, &guard)
        .await
        .unwrap_err();
    server.await.unwrap();

    assert!(error.to_string().contains("HOST_SLOT_MISMATCH"));
    assert_eq!(requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_slot_mismatch_prevents_new_session_rpc() {
    assert_slot_mismatch_sends_zero_rpc().await;
}

#[tokio::test]
async fn test_slot_mismatch_prevents_prompt_rpc() {
    assert_slot_mismatch_sends_zero_rpc().await;
}

#[tokio::test]
async fn test_verified_connection_is_the_connection_used_for_acp() {
    let temp = tempfile::TempDir::new().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = bind_server(&socket).await.unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let server_connections = connections.clone();
    let server = tokio::spawn(async move {
        let stream = listener.accept().await.unwrap();
        server_connections.fetch_add(1, Ordering::SeqCst);
        let transport = SocketTransport::from_stream(stream);
        while let Some(IncomingMessage::Request { id, method, .. }) = transport.recv().await {
            let value = match method.as_str() {
                "health" => json!({
                    "protocol_version": nexum_acp::transport::socket::LOCAL_PROTOCOL_VERSION,
                    "runtime_available": true,
                    "health": "ready"
                }),
                "test/connection_marker" => json!({"same_connection": true}),
                _ => panic!("unexpected method: {method}"),
            };
            transport.send_response(id, Ok(value)).await.unwrap();
        }
    });
    let guard = current_test_process_guard();

    let transport = connect_local_at_with_guard(AcpTransportMode::Local, &socket, &guard)
        .await
        .unwrap();
    let marker = transport
        .send_request("test/connection_marker", json!({}))
        .await
        .unwrap();

    assert_eq!(marker["same_connection"], true);
    assert_eq!(connections.load(Ordering::SeqCst), 1);
    transport.close().await.unwrap();
    drop(transport);
    server.await.unwrap();
}

#[tokio::test]
async fn test_guard_rejection_clears_loading_and_restores_prompt_once() {
    let temp = tempfile::TempDir::new().unwrap();
    let slot_a = temp.path().join("slot-A");
    let slot_b = temp.path().join("slot-B");
    std::fs::create_dir(&slot_a).unwrap();
    std::fs::create_dir(&slot_b).unwrap();
    let expected = slot_a.join("nexum-acp-host");
    let observed = slot_b.join("nexum-acp-host");
    std::fs::write(&expected, "A").unwrap();
    std::fs::write(&observed, "B").unwrap();
    let rejection = HostIdentityGuard::for_expected_host(&expected)
        .unwrap()
        .verify_observed_executable_for_test(48, &observed)
        .unwrap_err();
    let (mut app, _handle) = crate::app::App::new_headless(80, 24).await;
    {
        let mut cfg = app.services.nexum_config.write();
        cfg.config.active_provider_id = "guard-test".into();
        cfg.config.active_alias = "model-a".into();
        cfg.config.providers = vec![crate::config::ProviderConfig {
            id: "guard-test".into(),
            provider_type: "openai".into(),
            api_key: "test-key".into(),
            base_url: "https://example.invalid/v1".into(),
            ..Default::default()
        }];
    }
    let (client, notifications) = crate::acp_client::AcpTuiClient::new(
        rejected_host_transport_for_test(rejection.to_string()),
    );
    client.spawn_pump();
    app.session_mgr.current_mut().agent.acp_notification_rx = Some(notifications);
    app.acp_client = Some(client);

    app.submit_message("Hola".into());
    for _ in 0..100 {
        tokio::task::yield_now().await;
        app.poll_agent();
        if app.session_mgr.current().agent.turn_terminal.is_some() {
            break;
        }
    }

    app.handle_acp_notification(crate::acp_client::AcpNotification::TurnFailed {
        message: "duplicate terminal".into(),
    });

    assert!(!app.session_mgr.current().ui.loading);
    assert_eq!(
        app.session_mgr.current().ui.textarea.lines(),
        &["Hola".to_string()]
    );
    assert_eq!(app.session_mgr.current().agent.prompt_restoration_count, 1);
    assert_eq!(
        app.session_mgr.current().agent.turn_terminal,
        Some(nexum_acp::session::terminal::TerminalState::Failed)
    );
}

#[test]
fn test_guard_never_invokes_global_process_termination() {
    let source = include_str!("bootstrap.rs");
    for forbidden in ["pkill", "killall", "kill(-1", "Command::new(\"kill\""] {
        assert!(
            !source.contains(forbidden),
            "bootstrap must not use global process termination: {forbidden}"
        );
    }
}

struct SlotHostChild(Child);

impl Drop for SlotHostChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

async fn wait_child_success(child: &mut Child) -> bool {
    for _ in 0..100 {
        if let Some(status) = child.try_wait().unwrap() {
            return status.success();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

fn spawn_slot_host(slot: &Path, socket: &Path, count_path: &Path) -> SlotHostChild {
    spawn_slot_host_with_lifecycle(slot, socket, count_path, false)
}

fn spawn_durable_slot_host(slot: &Path, socket: &Path, count_path: &Path) -> SlotHostChild {
    spawn_slot_host_with_lifecycle(slot, socket, count_path, true)
}

fn spawn_slot_host_with_lifecycle(
    slot: &Path,
    socket: &Path,
    count_path: &Path,
    durable_until_sigterm: bool,
) -> SlotHostChild {
    std::fs::create_dir_all(slot).unwrap();
    let host = slot.join("nexum-acp-host");
    std::fs::copy(std::env::current_exe().unwrap(), &host).unwrap();
    let mut child = Command::new(&host)
        .arg("--exact")
        .arg("acp_client::bootstrap_test::host_identity_slot_host_helper")
        .arg("--ignored")
        .arg("--nocapture")
        .env("NEXUM_HOST_IDENTITY_TEST_SOCKET", socket)
        .env("NEXUM_HOST_IDENTITY_TEST_COUNT", count_path)
        .env(
            "NEXUM_HOST_IDENTITY_TEST_DURABLE",
            if durable_until_sigterm { "1" } else { "0" },
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if socket.exists() {
            return SlotHostChild(child);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("isolated slot host did not bind {}", socket.display());
}

#[test]
#[ignore = "helper process for the isolated SLOT_A/SLOT_B harness"]
fn host_identity_slot_host_helper() {
    let socket = PathBuf::from(std::env::var_os("NEXUM_HOST_IDENTITY_TEST_SOCKET").unwrap());
    let count_path = PathBuf::from(std::env::var_os("NEXUM_HOST_IDENTITY_TEST_COUNT").unwrap());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        #[cfg(unix)]
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        #[cfg(not(unix))]
        let mut term = NeverSignal;
        let listener = bind_server(&socket).await.unwrap();
        let mut count = 0_usize;
        let durable = std::env::var("NEXUM_HOST_IDENTITY_TEST_DURABLE").as_deref() == Ok("1");
        loop {
            let stream = if durable {
                tokio::select! {
                    accepted = listener.accept() => Some(accepted.unwrap()),
                    _ = term.recv() => None,
                }
            } else {
                Some(listener.accept().await.unwrap())
            };
            let Some(stream) = stream else {
                break;
            };
            let transport = SocketTransport::from_stream(stream);
            while let Some(message) = transport.recv().await {
                let IncomingMessage::Request { id, method, .. } = message else {
                    break;
                };
                count += 1;
                let response = match method.as_str() {
                    "health" => json!({
                        "protocol_version": nexum_acp::transport::socket::LOCAL_PROTOCOL_VERSION,
                        "runtime_available": true,
                        "health": "ready"
                    }),
                    "test/ping" => json!({"pong": true}),
                    _ => json!({"accepted": true}),
                };
                transport.send_response(id, Ok(response)).await.unwrap();
            }
            if !durable {
                break;
            }
        }
        let _ = std::fs::remove_file(&socket);
        std::fs::write(count_path, count.to_string()).unwrap();
    });
}

async fn assert_current_host_replaces_stale_host_after_graceful_owner_exit() {
    let temp = tempfile::TempDir::new().unwrap();
    let slot_current = temp.path().join("SLOT_CURRENT");
    let slot_stale = temp.path().join("SLOT_STALE");
    std::fs::create_dir(&slot_current).unwrap();
    std::fs::copy(
        std::env::current_exe().unwrap(),
        slot_current.join("nexum-acp-host"),
    )
    .unwrap();
    let socket = temp.path().join("recovery.sock");
    let stale_count = temp.path().join("stale.count");
    let current_count = temp.path().join("current.count");
    let mut stale = spawn_durable_slot_host(&slot_stale, &socket, &stale_count);
    let guard = HostIdentityGuard::for_expected_host(&slot_current.join("nexum-acp-host")).unwrap();
    let current_child = Arc::new(std::sync::Mutex::new(None));
    let current_child_for_spawn = current_child.clone();
    let slot_current_for_spawn = slot_current.clone();
    let socket_for_spawn = socket.clone();
    let current_count_for_spawn = current_count.clone();

    let transport = ensure_auto_host(&socket, Duration::from_secs(3), &guard, move || {
        *current_child_for_spawn.lock().unwrap() = Some(spawn_slot_host(
            &slot_current_for_spawn,
            &socket_for_spawn,
            &current_count_for_spawn,
        ));
        Ok(())
    })
    .await
    .unwrap();

    assert!(
        stale.0.wait().unwrap().success(),
        "stale host must handle SIGTERM and exit cleanly"
    );
    assert_eq!(
        std::fs::read_to_string(&stale_count).unwrap(),
        "0",
        "identity rejection must send no ACP RPC to the stale host"
    );
    let response = transport
        .send_request("test/ping", json!({}))
        .await
        .unwrap();
    assert_eq!(response["pong"], true);
    transport.close().await.unwrap();
    drop(transport);
    let mut current = current_child.lock().unwrap().take().unwrap();
    assert!(wait_child_success(&mut current.0).await);
    assert!(!socket.exists(), "socket owner must remove its own socket");
}

#[tokio::test]
async fn stale_host_is_terminated_gracefully() {
    assert_current_host_replaces_stale_host_after_graceful_owner_exit().await;
}

#[tokio::test]
async fn current_host_replaces_stale_host() {
    assert_current_host_replaces_stale_host_after_graceful_owner_exit().await;
}

async fn assert_expected_slot_host_starts_and_exits_without_orphan() {
    let temp = tempfile::TempDir::new().unwrap();
    let slot_a = temp.path().join("SLOT_A");
    let socket = temp.path().join("slot-a.sock");
    let count = temp.path().join("slot-a.count");
    let mut child = spawn_slot_host(&slot_a, &socket, &count);
    let guard = HostIdentityGuard::for_expected_host(&slot_a.join("nexum-acp-host")).unwrap();

    let transport = connect_local_at_with_guard(AcpTransportMode::Local, &socket, &guard)
        .await
        .unwrap();
    let response = transport
        .send_request("test/ping", json!({}))
        .await
        .unwrap();
    assert_eq!(response["pong"], true);
    transport.close().await.unwrap();
    drop(transport);
    assert!(child.0.wait().unwrap().success());
    assert_eq!(std::fs::read_to_string(count).unwrap(), "2");
    assert!(!socket.exists(), "exited host must remove its owned socket");
}

#[tokio::test]
async fn acp_host_starts_from_expected_slot() {
    assert_expected_slot_host_starts_and_exits_without_orphan().await;
}

#[tokio::test]
async fn no_orphan_host_after_tui_exit() {
    assert_expected_slot_host_starts_and_exits_without_orphan().await;
}

#[tokio::test]
async fn test_temp_slot_harness_rejects_slot_b_before_any_rpc() {
    let temp = tempfile::TempDir::new().unwrap();
    let slot_a = temp.path().join("SLOT_A");
    let slot_b = temp.path().join("SLOT_B");
    std::fs::create_dir(&slot_a).unwrap();
    std::fs::copy(
        std::env::current_exe().unwrap(),
        slot_a.join("nexum-acp-host"),
    )
    .unwrap();
    let socket = temp.path().join("slot-b.sock");
    let count = temp.path().join("slot-b.count");
    let mut child = spawn_slot_host(&slot_b, &socket, &count);
    let guard = HostIdentityGuard::for_expected_host(&slot_a.join("nexum-acp-host")).unwrap();

    let error = connect_local_at_with_guard(AcpTransportMode::Local, &socket, &guard)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("HOST_SLOT_MISMATCH"));
    assert!(child.0.wait().unwrap().success());
    assert_eq!(std::fs::read_to_string(count).unwrap(), "0");
}

async fn serve_shared_host(listener: LocalSocketListener, connections: usize) {
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..connections {
        let stream = listener.accept().await.unwrap();
        tasks.spawn(async move {
            let transport = SocketTransport::from_stream(stream);
            while let Some(IncomingMessage::Request { id, method, .. }) = transport.recv().await {
                let response = match method.as_str() {
                    "health" => json!({
                        "protocol_version": nexum_acp::transport::socket::LOCAL_PROTOCOL_VERSION,
                        "runtime_available": true,
                        "health": "ready"
                    }),
                    "runtime/identity" => json!({
                        "runtime_instance_id": "durable-host-1",
                        "provider": "openai",
                        "model": "mock-host-model"
                    }),
                    "runtime/capabilities" => json!({
                        "capabilities": { "hash": "shared-capabilities-hash" }
                    }),
                    _ => panic!("unexpected mock host method: {method}"),
                };
                transport.send_response(id, Ok(response)).await.unwrap();
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
}
