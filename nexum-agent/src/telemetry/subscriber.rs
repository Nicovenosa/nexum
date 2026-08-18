//! Tracing subscriber 初始化（基础日志输出）

use tracing_subscriber::{fmt, prelude::*, EnvFilter, Registry};

pub struct TracingGuard;

impl Drop for TracingGuard {
    fn drop(&mut self) {
        // 无需特殊清理逻辑
    }
}

/// 初始化 tracing，输出到日志文件（TUI 模式下避免干扰界面）
pub fn init_tracing(service_name: &str) -> TracingGuard {
    // 根据 RUST_LOG_FORMAT 环境变量决定输出格式
    let is_json = std::env::var("RUST_LOG_FORMAT").as_deref() == Ok("json");

    // 检查是否配置了日志文件
    let log_file = std::env::var("RUST_LOG_FILE").ok();

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        // 默认 info 级别，但 MCP 和插件模块设为 warn（避免连接日志干扰）
        EnvFilter::new("info,nexum_middlewares::mcp=warn,nexum_middlewares::plugin=warn,rmcp=warn")
    });

    match log_file {
        Some(path) => {
            // 输出到日志文件
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .expect("cannot open log file");

            if is_json {
                let subscriber = Registry::default()
                    .with(filter)
                    .with(fmt::layer().json().with_writer(file));
                tracing::subscriber::set_global_default(subscriber)
                    .expect("Unable to set global subscriber");
            } else {
                let subscriber = Registry::default()
                    .with(filter)
                    .with(fmt::layer().with_writer(file).with_ansi(false));
                tracing::subscriber::set_global_default(subscriber)
                    .expect("Unable to set global subscriber");
            }
        }
        None => {
            // TUI 应用默认将日志写到系统临时目录，避免干扰终端界面
            // ALLOW justificado: el nombre es por servicio y NO lleva PID a
            // propósito. Es el log que alguien va a buscar a mano cuando algo
            // falla, y un nombre que cambia en cada arranque lo vuelve
            // inencontrable. Se abre en modo append, así que varias instancias
            // lo comparten sin truncarse — que es distinto de dos escritores
            // sobre una SQLite.
            #[allow(clippy::disallowed_methods)]
            let default_path = std::env::temp_dir().join(format!("{}.log", service_name));
            let default_path = default_path.to_string_lossy().to_string();
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&default_path)
                .expect("cannot open default log file");

            if is_json {
                let subscriber = Registry::default()
                    .with(filter)
                    .with(fmt::layer().json().with_writer(file));
                tracing::subscriber::set_global_default(subscriber)
                    .expect("Unable to set global subscriber");
            } else {
                let subscriber = Registry::default()
                    .with(filter)
                    .with(fmt::layer().with_writer(file).with_ansi(false));
                tracing::subscriber::set_global_default(subscriber)
                    .expect("Unable to set global subscriber");
            }
        }
    }

    TracingGuard
}
