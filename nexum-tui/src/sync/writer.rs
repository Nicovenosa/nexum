//! Escritura de items sincronizados al filesystem local.
//!
//! # DEUDA: esto NO es transaccional
//!
//! Auditado el 2026-08-01. Lo que está bien: las seis escrituras son atómicas
//! individualmente —`write` a `.tmp` + `rename`—, así que ningún archivo queda
//! a medias. Lo que está mal es todo lo que pasa entre ellas.
//!
//! ## 1. Atomicidad entre artefactos: no existe
//!
//! `settings` + `skills` + `mcp` + `plugins` son UNA configuración, no cuatro
//! archivos sueltos: un skill puede depender de un servidor MCP declarado en
//! `.mcp.json`, un plugin puede requerir claves de `settings.json`. Pero acá son
//! seis escrituras en secuencia sin nada que las una. Si el proceso muere —o si
//! un `?` propaga un error, que es más probable— después de settings y antes de
//! mcp, el usuario queda con settings nuevos y MCP viejo, sin marca de que está
//! a medias y sin rollback.
//!
//! Es la misma forma que ya nos costó siete bugs, con otra cara: artefactos que
//! tienen que estar sincronizados y ningún mecanismo que grite cuando se
//! separan. Acá ni siquiera hay un gate que lo detecte después.
//!
//! ## 2. Concurrencia: cero coordinación
//!
//! No hay locking de ningún tipo. Dos `write_sync_items` en paralelo —una sync
//! local mientras entra una remota, o dos instancias de Nexum— producen una
//! mezcla: settings de un origen, skills de otro.
//!
//! ## 3. El `.bak` se destruye a sí mismo bajo concurrencia
//!
//! El backup de `settings.json` va a un nombre FIJO, `settings.json.bak`. Si dos
//! syncs se cruzan, el segundo copia a `.bak` el settings que acaba de escribir
//! el primero: el respaldo deja de ser el estado previo del usuario y pasa a ser
//! el estado intermedio de otro proceso.
//!
//! **Un backup que a veces es el estado previo y a veces el intermedio de otro
//! sync es peor que no tener backup**, porque se usa creyendo que es lo primero.
//! Al arreglarlo, el nombre tiene que llevar timestamp o la marca del sync que
//! lo produjo — nunca un nombre fijo.
//!
//! ## Cómo se arregla, decidido ANTES de implementar
//!
//! Staging: escribir el conjunto completo a un directorio temporal, validarlo
//! entero, y recién ahí publicar. Más un lock de proceso para el cruce.
//!
//! **Si el conjunto no valida, se descarta el sync ENTERO y queda lo viejo
//! intacto.** Falla cerrada, no aplicación parcial con aviso. Es lo consistente
//! con el resto del proyecto —la estampa de generación, los gates de
//! empaquetado, el aislamiento de métricas— y queda escrito acá para que no se
//! decida en el medio de la implementación, que es cuando la salida fácil
//! ("aplicá lo que validó y avisá del resto") parece razonable y deja al usuario
//! con una configuración mezclada que nadie pidió.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use crate::sync::protocol::{FileEntry, SyncItems};

/// 文件写入错误类型
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// 路径穿越攻击或非法路径
    #[error("路径穿越攻击：{0}")]
    PathTraversal(String),
    /// 文件 I/O 错误
    #[error("文件写入失败：{0}")]
    Io(#[from] std::io::Error),
    /// Otro sync está corriendo. FALLA CERRADA: no se mezcla con el ajeno.
    #[error("otro sync está en curso (pid {0}); no se aplica nada")]
    Concurrente(i32),
    /// El conjunto preparado no validó. No se publicó NADA.
    #[error("el conjunto no validó: {0}. No se aplicó ningún cambio")]
    ConjuntoInvalido(String),
}

/// 规范化路径：消除 . 和 .. 组件，返回纯绝对路径
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => {
                result.push(other);
            }
        }
    }
    result
}

/// 验证相对路径安全并解析为绝对路径
///
/// 安全检查：
/// 1. 拒绝绝对路径（Unix / 开头、Windows C:\ 或 \\ 开头）
/// 2. 拒绝包含 .. 父目录组件的路径（深度计数器检测）
/// 3. 解析后验证最终路径仍以 base_dir 为前缀（兜底防护）
pub fn validate_and_resolve(base_dir: &Path, relative_path: &str) -> Result<PathBuf, WriteError> {
    // Step 1: 拒绝绝对路径
    let rel = Path::new(relative_path);
    if rel.is_absolute()
        || relative_path.starts_with('/')
        || relative_path.starts_with('\\')
        || (relative_path.len() > 2 && relative_path.as_bytes()[1] == b':')
    {
        tracing::warn!("拒绝绝对路径或非法路径前缀: {}", relative_path);
        return Err(WriteError::PathTraversal(format!(
            "绝对路径被拒绝: {}",
            relative_path
        )));
    }

    // Step 2: 逐组件检查 —— 拒绝任何 ParentDir 组件
    let mut depth: i32 = 0;
    for component in rel.components() {
        match component {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    tracing::warn!("路径穿越攻击拒绝: {} (base: {:?})", relative_path, base_dir);
                    return Err(WriteError::PathTraversal(format!(
                        "路径包含 .. 穿越: {}",
                        relative_path
                    )));
                }
            }
            Component::Normal(_) => depth += 1,
            _ => {} // RootDir 和 Prefix 已在 is_absolute() 中拒绝
        }
    }

    // Step 3: 解析绝对路径并验证仍在 base_dir 内（兜底）
    let resolved = base_dir.join(rel);
    let normalized = normalize_path(&resolved);
    if !normalized.starts_with(base_dir) {
        tracing::warn!(
            "路径解析后逃逸 base_dir: {:?} (base: {:?})",
            normalized,
            base_dir
        );
        return Err(WriteError::PathTraversal(format!(
            "路径逃逸 base_dir: {}",
            relative_path
        )));
    }

    Ok(normalized)
}

/// 向 base_dir 下安全写入一个 FileEntry
///
/// 自动创建父目录，原子写入（写临时文件 → rename）
pub fn write_file_entry(base_dir: &Path, entry: &FileEntry) -> Result<(), WriteError> {
    let target_path = validate_and_resolve(base_dir, &entry.path)?;

    // 确保父目录存在
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // 原子写入：先写临时文件，再 rename
    let tmp_path = target_path.with_extension("tmp");
    fs::write(&tmp_path, &entry.content)?;
    fs::rename(&tmp_path, &target_path)?;

    tracing::info!("已写入文件: {:?}", target_path);
    Ok(())
}

/// 将同步项写入本地文件系统
///
/// 路径映射：
/// - settings → {home_dir}/.peri/settings.json（先备份为 .bak）
/// - skills   → {home_dir}/.claude/skills/{relative_path}
/// - mcp      → {home_dir}/.mcp.json + {cwd}/.mcp.json（如有）
/// - plugins  → {home_dir}/.claude/plugins/cache/{relative_path}
/// Una escritura ya decidida: destino final y contenido. Sin tocar disco.
#[derive(Debug, Clone)]
struct Planificada {
    destino: PathBuf,
    contenido: Vec<u8>,
    /// Etiqueta legible para los mensajes de error.
    que: String,
}

/// Traduce los items a escrituras concretas SIN tocar el disco.
///
/// Toda la validación de rutas pasa acá. Que falle en esta fase significa que
/// no se escribió nada — que es el punto de la falla cerrada.
fn planificar(
    home_dir: &Path,
    cwd: &Path,
    items: &SyncItems,
) -> Result<Vec<Planificada>, WriteError> {
    let mut plan = Vec::new();

    if let Some(ref settings) = items.settings {
        plan.push(Planificada {
            destino: home_dir.join(".peri").join("settings.json"),
            contenido: settings.content.as_bytes().to_vec(),
            que: "settings.json".into(),
        });
        if let Some(ref claude) = settings.claude_content {
            plan.push(Planificada {
                destino: home_dir.join(".claude").join("settings.json"),
                contenido: claude.as_bytes().to_vec(),
                que: ".claude/settings.json".into(),
            });
        }
    }
    if let Some(ref skills) = items.skills {
        let base = home_dir.join(".claude").join("skills");
        for e in &skills.files {
            plan.push(Planificada {
                destino: validate_and_resolve(&base, &e.path)?,
                contenido: e.content.clone(),
                que: format!("skills/{}", e.path),
            });
        }
    }
    if let Some(ref mcp) = items.mcp {
        if let Some(ref g) = mcp.global {
            plan.push(Planificada {
                destino: home_dir.join(".mcp.json"),
                contenido: g.as_bytes().to_vec(),
                que: ".mcp.json (global)".into(),
            });
        }
        if let Some(ref pr) = mcp.project {
            plan.push(Planificada {
                destino: cwd.join(".mcp.json"),
                contenido: pr.as_bytes().to_vec(),
                que: ".mcp.json (proyecto)".into(),
            });
        }
    }
    if let Some(ref plugins) = items.plugins {
        let base = home_dir.join(".claude").join("plugins").join("cache");
        for e in &plugins.files {
            plan.push(Planificada {
                destino: validate_and_resolve(&base, &e.path)?,
                contenido: e.content.clone(),
                que: format!("plugins/{}", e.path),
            });
        }
    }
    Ok(plan)
}

/// Lock de proceso para todo el sync.
///
/// Sin esto, dos syncs simultáneos mezclan orígenes —settings de uno, skills del
/// otro— y el segundo copia al backup lo que acaba de escribir el primero, así
/// que el rescate se destruye a sí mismo.
struct LockSync(PathBuf);

impl LockSync {
    fn tomar(home_dir: &Path) -> Result<Self, WriteError> {
        let path = home_dir.join(".peri").join("sync.lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = write!(f, "{}", std::process::id());
                    return Ok(Self(path));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let pid: i32 = fs::read_to_string(&path)
                        .ok()
                        .and_then(|s| s.trim().parse().ok())
                        .unwrap_or(0);
                    // Un lock de un proceso muerto no puede bloquear para
                    // siempre: se limpia y se reintenta una vez. La liveness se
                    // consulta con sysinfo (portátil: /proc no existe fuera de
                    // Linux).
                    let vivo = pid > 0 && {
                        use sysinfo::{Pid, ProcessesToUpdate, System};
                        let mut system = System::new();
                        system.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid as u32)]), true);
                        system.process(Pid::from_u32(pid as u32)).is_some()
                    };
                    if vivo {
                        return Err(WriteError::Concurrente(pid));
                    }
                    let _ = fs::remove_file(&path);
                }
                Err(e) => return Err(WriteError::Io(e)),
            }
        }
    }
}

impl Drop for LockSync {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn marca_temporal() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Escribe los items sincronizados de forma TRANSACCIONAL.
///
/// Antes eran seis escrituras atómicas individuales sin atomicidad entre ellas y
/// sin locking: una interrupción dejaba settings nuevos con MCP viejo, sin marca
/// ni rollback. Ahora:
///
/// 1. **Planificar** — se traduce todo a (destino, contenido) sin tocar disco.
///    Las rutas se validan acá: si algo falla, no se escribió nada.
/// 2. **Preparar** — todo va a un directorio de staging por PID.
/// 3. **Validar** — se verifica que el conjunto esté completo y legible.
/// 4. **Respaldar** — los destinos existentes se copian con TIMESTAMP.
/// 5. **Publicar** — rename de cada archivo desde staging.
///
/// FALLA CERRADA: si el conjunto no valida, se descarta el sync entero y queda
/// lo viejo intacto. Nunca aplicación parcial con aviso — esa salida parece
/// razonable en el momento y deja al usuario con una configuración mezclada que
/// nadie pidió.
pub fn write_sync_items(home_dir: &Path, cwd: &Path, items: &SyncItems) -> Result<(), WriteError> {
    let _lock = LockSync::tomar(home_dir)?;

    // 1. Planificar. Falla acá = no se tocó nada.
    let plan = planificar(home_dir, cwd, items)?;
    if plan.is_empty() {
        return Ok(());
    }

    // 2. Preparar en staging.
    let staging = home_dir
        .join(".peri")
        .join(format!("sync-staging-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let guard = StagingGuard(staging.clone());

    let mut preparadas: Vec<(PathBuf, &Planificada)> = Vec::new();
    for (i, p) in plan.iter().enumerate() {
        let tmp = staging.join(format!("{i}.blob"));
        fs::write(&tmp, &p.contenido)?;
        preparadas.push((tmp, p));
    }

    // 3. Validar el CONJUNTO antes de publicar nada.
    for (tmp, p) in &preparadas {
        let leido = fs::read(tmp).map_err(|e| {
            WriteError::ConjuntoInvalido(format!("{} no se pudo releer: {e}", p.que))
        })?;
        if leido.len() != p.contenido.len() {
            return Err(WriteError::ConjuntoInvalido(format!(
                "{} quedó incompleto en staging ({} de {} bytes)",
                p.que,
                leido.len(),
                p.contenido.len()
            )));
        }
    }

    // 4. Respaldar lo existente, con TIMESTAMP.
    //
    // El `.bak` de nombre fijo se destruía a sí mismo: con dos syncs cruzados,
    // el segundo copiaba a `.bak` lo que acababa de escribir el primero, y el
    // respaldo pasaba a ser el estado intermedio de otro proceso en vez del
    // previo del usuario. Un backup que a veces es una cosa y a veces otra es
    // peor que no tenerlo, porque se usa creyendo que es lo primero.
    let sello = marca_temporal();
    for (_, p) in &preparadas {
        if p.destino.is_file() {
            let bak = p.destino.with_extension(format!("bak.{sello}"));
            let _ = fs::copy(&p.destino, &bak);
        }
    }

    // 5. Publicar.
    for (tmp, p) in &preparadas {
        if let Some(parent) = p.destino.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(tmp, &p.destino)?;
    }

    drop(guard);
    tracing::info!(archivos = plan.len(), sello, "sync publicado");
    Ok(())
}

/// Borra el staging pase lo que pase, incluso si se sale por `?`.
struct StagingGuard(PathBuf);

impl Drop for StagingGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
