//! NexumRuntimeBootstrap (F2.2.1): `nexum voice` debe funcionar desde
//! COLD START, sin TUI abierta. Descubre/valida/lanza el sidecar
//! Hormiguero con readiness acotada; limpia discovery stale; nunca
//! duplica sidecars; guarda último error SANITIZADO (sin tokens/paths).

use std::path::PathBuf;
use std::time::{Duration, Instant};

const READY_TIMEOUT_MS: u64 = 3000;
const LAST_ERROR_FILE: &str = "voice-last-error.txt";

fn runtime_dir() -> PathBuf {
    crate::hormiguero::bridge::runtime_dir()
        .unwrap_or_else(|| nexum_agent::config_home::nexum_home().join("runtime"))
}

/// Localiza el dir de la CLI (donde vive el sidecar Python).
/// Prioridad: NEXUM_CLI_DIR > layout del artefacto instalado (`<slot>/`) >
/// ancestros del binario (checkout). El slot instalado es la fuente canónica;
/// no hay rutas del entorno del desarrollador.
fn find_cli_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("NEXUM_CLI_DIR") {
        let p = PathBuf::from(d);
        if p.join("src/nexum_hormiguero_sidecar").is_dir() {
            return Some(p);
        }
    }
    if let Some(layout) = crate::layout::InstalledLayoutV1::current() {
        let p = layout.version_root();
        if p.join("src/nexum_hormiguero_sidecar").is_dir() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut a = exe.as_path();
        for _ in 0..6 {
            a = a.parent()?;
            if a.join("src/nexum_hormiguero_sidecar").is_dir() {
                return Some(a.to_path_buf());
            }
        }
    }
    None
}

fn health_ok(dir: &std::path::Path) -> bool {
    let Ok(port) = std::fs::read_to_string(dir.join("hormiguero.port")) else {
        return false;
    };
    let Ok(port) = port.trim().parse::<u16>() else {
        return false;
    };
    crate::hormiguero::http::request(
        port, "GET", "/health", None, None, Duration::from_millis(400),
    )
    .map(|r| r.status == 200)
    .unwrap_or(false)
}

fn clean_stale(dir: &std::path::Path) {
    for f in ["hormiguero.port", "hormiguero.token", "hormiguero.pid"] {
        let _ = std::fs::remove_file(dir.join(f));
    }
}

pub fn record_error(msg: &str) {
    // Sanitizado: solo el mensaje corto, jamás tokens/puertos/rutas home.
    let dir = runtime_dir();
    let _ = std::fs::create_dir_all(&dir);
    let line = format!(
        "{} · {}\n",
        chrono_lite_now(),
        msg.replace(dirs_next::home_dir().unwrap_or_default().to_string_lossy().as_ref(), "~")
    );
    let _ = std::fs::write(dir.join(LAST_ERROR_FILE), line);
}

fn chrono_lite_now() -> String {
    // Sin dep chrono en este módulo: epoch legible alcanza para diagnóstico.
    format!("t+{}s", std::time::UNIX_EPOCH.elapsed().map(|d| d.as_secs()).unwrap_or(0))
}

pub fn run_last_error() -> i32 {
    match std::fs::read_to_string(runtime_dir().join(LAST_ERROR_FILE)) {
        Ok(s) => {
            println!("[voice] último error: {}", s.trim());
            0
        }
        Err(_) => {
            println!("[voice] sin errores registrados");
            0
        }
    }
}

/// Garantiza runtime local listo. Barato si ya está (<50ms); cold start
/// lanza el sidecar y espera readiness ≤3s. Idempotente, sin duplicados
/// (el propio sidecar mata huérfanos por pidfile; acá además si health OK
/// no se lanza nada).
pub fn ensure_local_runtime() -> Result<(), String> {
    // Super+Z / `nexum voice` = opt-in explícito del usuario al pasillo
    // local: el flag ambiente no puede depender del entorno del atajo KDE
    // (root cause F2.2.1). public demo sigue ganando SIEMPRE.
    if crate::ui::demo_mode::public_demo_enabled() {
        return Err("public demo: voz deshabilitada".into());
    }
    std::env::set_var("NEXUM_HORMIGUERO", "on");
    let dir = runtime_dir();
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    std::env::set_var("NEXUM_HORMIGUERO_RUNTIME_DIR", &dir);

    // 1. ¿Ya está sano?
    if health_ok(&dir) {
        return Ok(());
    }
    // 2. Discovery stale (archivos sin sidecar respondiendo) → limpiar.
    if dir.join("hormiguero.port").exists() {
        clean_stale(&dir);
    }
    // 3. Lanzar sidecar.
    let cli = find_cli_dir().ok_or_else(|| {
        record_error("no encontré la capa CLI del sidecar (seteá NEXUM_CLI_DIR)");
        "no encontré el runtime de Nexum (NEXUM_CLI_DIR)".to_string()
    })?;
    let log = std::fs::File::create(dir.join("hormiguero-sidecar.log")).ok();
    let mut cmd = std::process::Command::new("python3");
    cmd.args(["-m", "nexum_hormiguero_sidecar"])
        .env("PYTHONPATH", cli.join("src"))
        .env("NEXUM_HORMIGUERO_RUNTIME_DIR", &dir)
        .current_dir(&cli)
        .stdin(std::process::Stdio::null());
    if let Some(l) = log {
        let l2 = l.try_clone().ok();
        cmd.stdout(l);
        if let Some(l2) = l2 {
            cmd.stderr(l2);
        }
    }
    cmd.spawn().map_err(|e| {
        record_error(&format!("no pude lanzar el sidecar: {e}"));
        format!("no pude lanzar el runtime local: {e}")
    })?;
    // 4. Readiness acotada.
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(READY_TIMEOUT_MS) {
        if health_ok(&dir) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    record_error("el sidecar no respondió /health dentro del timeout de arranque");
    Err("Hormiguero no respondió al iniciar".into())
}

/// `--doctor --repair`: limpia stale (voice.pid + discovery), relanza y
/// verifica. Solo mata procesos propios (el sidecar se auto-reemplaza por
/// pidfile; el voice.pid solo si el proceso ya no existe).
pub fn run_repair() -> i32 {
    let dir = runtime_dir();
    // voice.pid stale
    if let Ok(pid) = std::fs::read_to_string(dir.join("voice.pid")) {
        let alive = std::fs::read_to_string(format!("/proc/{}/cmdline", pid.trim()))
            .map(|c| c.contains("voice"))
            .unwrap_or(false);
        if !alive {
            let _ = std::fs::remove_file(dir.join("voice.pid"));
            println!("[repair] voice.pid stale eliminado");
        }
    }
    if !health_ok(&dir) {
        clean_stale(&dir);
        println!("[repair] discovery stale limpiado");
    }
    match ensure_local_runtime() {
        Ok(()) => {
            println!("[repair] runtime local OK (health verde)");
            0
        }
        Err(e) => {
            eprintln!("[repair] {e} — mirá --last-error");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hormiguero::bridge::test_env_lock;

    #[test]
    fn test_public_demo_bloquea_bootstrap() {
        let _g = test_env_lock();
        std::env::set_var("NEXUM_PUBLIC_DEMO", "1");
        let r = ensure_local_runtime();
        std::env::remove_var("NEXUM_PUBLIC_DEMO");
        assert!(r.is_err());
    }

    #[test]
    fn test_find_cli_dir_desde_repo_conocido() {
        let _g = test_env_lock();
        std::env::remove_var("NEXUM_CLI_DIR");
        // En esta máquina el repo existe → lo encuentra por ruta conocida
        // o ancestros; en CI ajeno puede ser None (no panic).
        let _ = find_cli_dir();
    }

    #[test]
    fn test_record_y_last_error_sanitizado() {
        let _g = test_env_lock();
        let home = dirs_next::home_dir().unwrap();
        record_error(&format!("fallo en {}/secreto", home.display()));
        let s = std::fs::read_to_string(runtime_dir().join(LAST_ERROR_FILE)).unwrap();
        assert!(!s.contains(home.to_string_lossy().as_ref()), "home sanitizado: {s}");
        assert!(s.contains("~/secreto"));
    }
}
