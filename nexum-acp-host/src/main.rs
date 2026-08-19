#[cfg(unix)]
mod config;
#[cfg(unix)]
mod host;
#[cfg(all(test, unix))]
mod host_test;
#[cfg(unix)]
mod instance_lock;
#[cfg(unix)]
mod lifecycle;
#[cfg(all(test, unix))]
mod lifecycle_test;

#[cfg(unix)]
use clap::Parser;

#[cfg(unix)]
#[derive(Parser)]
#[command(name = "nexum-acp-host", version, about = "Local ACP Unix socket host")]
struct Cli {
    /// Socket override for tests or isolated manual instances.
    #[arg(long)]
    socket: Option<std::path::PathBuf>,
    /// Sanitized lifecycle diagnostic written with mode 0600.
    #[arg(long)]
    diagnostic: Option<std::path::PathBuf>,
}

/// This binary hosts a Unix-domain-socket ACP server and is Unix-only.
#[cfg(not(unix))]
fn main() {
    eprintln!("nexum-acp-host: not supported on this platform (Unix domain sockets required)");
    std::process::exit(1);
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(path) = cli.diagnostic.clone() {
        install_panic_diagnostic(path);
    }
    // Always validate the process runtime before accepting either the default
    // or an explicit test socket; an override must not bypass isolation.
    let default_socket = lifecycle::default_socket_path()?;
    let socket_path = cli.socket.unwrap_or(default_socket);
    let result = run(socket_path).await;
    if let Some(path) = cli.diagnostic {
        let (classification, exit_code, message) = match &result {
            Ok(()) => ("HOST_CLEAN_EXIT", 0, "graceful shutdown".to_string()),
            Err(error) => (
                "HOST_INITIALIZATION_OR_RUNTIME_FAILURE",
                1,
                sanitize_diagnostic(error),
            ),
        };
        let _ = write_diagnostic(&path, classification, exit_code, &message);
    }
    result
}

#[cfg(unix)]
async fn run(socket_path: std::path::PathBuf) -> anyhow::Result<()> {
    // Guard de instancia única (Fase B): si ya hay un host autoritativo sobre
    // este socket, salir limpio (exit 0) SIN duplicar. Elimina la acumulación
    // de hosts durables (F14). El guard se mantiene vivo toda la sesión.
    let _instance = match instance_lock::acquire(&socket_path)? {
        Some(guard) => guard,
        None => {
            // Otro host ya sirve este socket; no arrancamos un segundo.
            return Ok(());
        }
    };

    let config = config::load_host_config().await?;
    host::run(socket_path, config).await
}

#[cfg(unix)]
fn sanitize_diagnostic(error: &anyhow::Error) -> String {
    let raw = format!("{error:#}");
    let lowered = raw.to_ascii_lowercase();
    if [
        "apikey",
        "api_key",
        "token",
        "secret",
        "password",
        "cookie",
        "credential",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
    {
        "sensitive diagnostic redacted".to_string()
    } else {
        raw.chars().take(240).collect()
    }
}

#[cfg(unix)]
fn write_diagnostic(
    path: &std::path::Path,
    classification: &str,
    exit_code: i32,
    message: &str,
) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({
            "classification": classification,
            "exit_code": exit_code,
            "message": message,
        }),
    )?;
    file.write_all(b"\n")
}

#[cfg(unix)]
fn install_panic_diagnostic(path: std::path::PathBuf) {
    std::panic::set_hook(Box::new(move |_| {
        let _ = write_diagnostic(&path, "HOST_CRASH", 101, "panic payload redacted");
    }));
}
