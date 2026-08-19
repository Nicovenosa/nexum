//! Guard de instancia única del ACP host (OMEGA Live Wiring Iter 3, Fase B).
//!
//! Elimina de raíz la acumulación de hosts durables: el host adquiere un
//! `flock(LOCK_EX|LOCK_NB)` exclusivo sobre `<socket>.lock` y lo mantiene toda
//! su vida. Un segundo host que intente arrancar sobre el mismo socket obtiene
//! `EWOULDBLOCK` y sale limpio (exit 0) SIN duplicarse. Al morir el host (por
//! cualquier causa: exit, SIGTERM, SIGKILL, panic), el SO libera el flock
//! automáticamente, de modo que el próximo host puede tomarlo. No hay huérfanos,
//! no hay zombies, no hay crecimiento de procesos.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// Mantiene el flock exclusivo durante la vida del proceso. Al dropearse (o al
/// terminar el proceso) el SO libera el lock.
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Ruta del lock para un socket dado (`<socket>.lock`). Atada al socket para que
/// instancias de test con sockets propios no colisionen con el host default.
pub fn lock_path_for(socket_path: &Path) -> PathBuf {
    let mut s = socket_path.as_os_str().to_owned();
    s.push(".lock");
    PathBuf::from(s)
}

/// Intenta adquirir el lock de instancia única para `socket_path`.
///   `Ok(Some(guard))` — este proceso es el host autoritativo;
///   `Ok(None)`        — ya hay un host vivo (otro proceso tiene el lock);
///   `Err(_)`          — error de IO real.
pub fn acquire(socket_path: &Path) -> std::io::Result<Option<InstanceLock>> {
    let lock_path = lock_path_for(socket_path);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    let fd = file.as_raw_fd();
    // flock exclusivo, no bloqueante.
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        // EWOULDBLOCK/EAGAIN ⇒ otro host ya lo tiene (no es error).
        if matches!(err.raw_os_error(), Some(libc::EWOULDBLOCK)) {
            return Ok(None);
        }
        return Err(err);
    }
    // Somos el dueño: registrar PID para forense (truncar + escribir).
    let mut f = &file;
    let _ = f.set_len(0);
    let _ = write!(f, "{}", std::process::id());
    Ok(Some(InstanceLock {
        _file: file,
        path: lock_path,
    }))
}

#[cfg(test)]
#[path = "instance_lock_test.rs"]
mod instance_lock_test;
