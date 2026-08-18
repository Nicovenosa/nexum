use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context;
use nexum_acp::{
    cron::{CronRuntime, ExecutePromptRunner, HeadlessPromptContextFactory},
    server::run_acp_server,
    transport::{hub::AcpHub, types::HostPrincipal, unix::UnixTransport, AcpTransport},
};
use tokio::net::{UnixListener, UnixStream};

use crate::{config::HostConfig, lifecycle};

pub async fn run(socket_path: PathBuf, config: HostConfig) -> anyhow::Result<()> {
    let HostConfig { server, cron } = config;
    let cron_runtime = if let Some(cron) = cron {
        let cron_context = Arc::new(HeadlessPromptContextFactory::new(cron.prompt_resources));
        let cron_runner = Arc::new(ExecutePromptRunner::new(cron_context));
        let runtime = Arc::new(CronRuntime::new(cron.store, cron_runner));
        runtime
            .start(Duration::from_secs(15))
            .context("start local cron scheduler")?;
        Some(runtime)
    } else {
        None
    };

    let listener = lifecycle::bind(&socket_path)
        .await
        .context("bind local ACP socket")?;
    let socket_inode =
        lifecycle::socket_inode(&socket_path).context("identify owned local ACP socket")?;
    let hub = Arc::new(AcpHub::new(8));
    #[cfg(not(target_os = "linux"))]
    tracing::warn!(
        "local peer credentials unavailable; durable cron interaction resolution disabled"
    );
    let server_transport: Arc<dyn AcpTransport> = hub.clone();
    tokio::spawn(async move { run_acp_server(server_transport, server).await });
    tracing::info!(socket = %socket_path.display(), "local ACP host ready");

    // Apagado graceful: en SIGTERM/SIGINT salimos del accept loop para limpiar
    // el socket (stale_sockets=0). El flock de instancia única lo libera el SO
    // al morir el proceso, pase lo que pase.
    let result = tokio::select! {
        r = accept_loop(listener, hub) => r,
        _ = shutdown_signal() => {
            tracing::info!("local ACP host: señal de apagado recibida");
            Ok(())
        }
    };
    if let Some(runtime) = cron_runtime {
        runtime.stop();
    }
    // Cleanup only the inode this process actually bound. A successor that
    // already rebound the pathname is never unlinked by the exiting owner.
    let _ = lifecycle::remove_owned_socket(&socket_path, socket_inode);
    result
}

/// Espera una señal de apagado (SIGTERM/SIGINT/SIGHUP en unix; Ctrl+C en otros).
/// SIGHUP se captura para que el cierre del PTY/terminal limpie el socket en vez
/// de terminar el proceso con disposición default (que dejaría el socket stale).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => return futures_pending().await,
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(_) => return futures_pending().await,
        };
        let mut hup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(_) => return futures_pending().await,
        };
        tokio::select! {
            _ = term.recv() => {},
            _ = int.recv() => {},
            _ = hup.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Nunca resuelve (fallback si no se pueden instalar handlers de señal).
async fn futures_pending() {
    std::future::pending::<()>().await
}

async fn accept_loop(listener: UnixListener, hub: Arc<AcpHub>) -> anyhow::Result<()> {
    loop {
        let (stream, _) = listener.accept().await.context("accept local ACP client")?;
        let principal = match unix_peer_principal(&stream) {
            Ok(principal) => principal,
            Err(error) => {
                tracing::warn!(error = %error, "local ACP peer rejected without durable principal");
                continue;
            }
        };
        hub.attach_with_principal(Arc::new(UnixTransport::from_stream(stream)), principal)?;
    }
}

#[cfg(target_os = "linux")]
fn unix_peer_principal(stream: &UnixStream) -> anyhow::Result<HostPrincipal> {
    let peer = stream.peer_cred().context("read local peer credentials")?;
    if peer.uid() != unsafe { libc::geteuid() } {
        anyhow::bail!("local ACP peer UID mismatch");
    }
    HostPrincipal::new(format!("unix-uid:{}", peer.uid())).map_err(Into::into)
}

#[cfg(not(target_os = "linux"))]
fn unix_peer_principal(_stream: &UnixStream) -> anyhow::Result<HostPrincipal> {
    anyhow::bail!("local peer credentials are unavailable on this platform")
}
