use nexum_web_pty::config::Config;
use nexum_web_pty::start_server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().try_init().ok();
    start_server(Config::from_args()).await
}
