#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::TempDir;

    use crate::sync::{
        protocol::{FileEntry, FilesItem, McpItem, SettingsItem, SyncItems},
        writer,
    };

    #[test]
    fn test_validate_normal_relative_path() {
        let base = Path::new("/tmp/base");
        let result = writer::validate_and_resolve(base, "my-skill/SKILL.md");
        assert!(result.is_ok(), "正常相对路径应通过");
        let resolved = result.unwrap();
        assert!(resolved.starts_with(base));
        assert!(resolved.ends_with("my-skill/SKILL.md"));
    }

    #[test]
    fn test_validate_rejects_absolute_path() {
        let base = Path::new("/tmp/base");
        let result = writer::validate_and_resolve(base, "/etc/passwd");
        assert!(result.is_err(), "绝对路径应被拒绝");
        match result.unwrap_err() {
            writer::WriteError::PathTraversal(_) => {}
            _ => panic!("应返回 PathTraversal 错误"),
        }
    }

    #[test]
    fn test_validate_rejects_parent_dir_traversal() {
        let base = Path::new("/tmp/base");
        let result = writer::validate_and_resolve(base, "../.ssh/authorized_keys");
        assert!(result.is_err(), "../ 穿越应被拒绝");
    }

    #[test]
    fn test_validate_rejects_hidden_traversal() {
        let base = Path::new("/tmp/base");
        // foo/../../bar → depth: 1 → 0 → -1
        let result = writer::validate_and_resolve(base, "foo/../../bar");
        assert!(result.is_err(), "foo/../../bar 穿越应被拒绝");
    }

    #[test]
    fn test_write_file_entry_creates_parent_dirs() {
        let tmp = TempDir::new().expect("创建临时目录");
        let base = tmp.path();
        let entry = FileEntry {
            path: "a/b/c.txt".into(),
            content: b"hi".to_vec(),
        };
        writer::write_file_entry(base, &entry).expect("写入应成功");
        let written = base.join("a/b/c.txt");
        assert!(written.exists(), "文件应被创建");
        assert_eq!(fs::read_to_string(&written).unwrap(), "hi");
    }

    #[test]
    fn test_write_file_entry_rejects_traversal() {
        let tmp = TempDir::new().expect("创建临时目录");
        let base = tmp.path();
        let entry = FileEntry {
            path: "../bad.txt".into(),
            content: b"x".to_vec(),
        };
        let result = writer::write_file_entry(base, &entry);
        assert!(result.is_err(), "路径穿越应被拒绝");
    }

    #[test]
    fn test_write_sync_items_settings_with_backup() {
        let home = TempDir::new().expect("创建临时 home");
        let cwd = TempDir::new().expect("创建临时 cwd");
        let home_p = home.path();
        let cwd_p = cwd.path();

        // 创建预先存在的 settings.json
        let nexum_dir = home_p.join(".peri");
        fs::create_dir_all(&nexum_dir).unwrap();
        fs::write(nexum_dir.join("settings.json"), "old").unwrap();

        let items = SyncItems {
            settings: Some(SettingsItem {
                content: "new".into(),
                claude_content: None,
            }),
            skills: None,
            mcp: None,
            plugins: None,
        };

        writer::write_sync_items(home_p, cwd_p, &items).expect("写入应成功");

        // 新文件内容为 "new"
        assert_eq!(
            fs::read_to_string(home_p.join(".peri/settings.json")).unwrap(),
            "new"
        );
        // El backup contiene "old", pero YA NO con nombre fijo: lleva sello.
        //
        // Este test assertaba `settings.json.bak` a secas, que es el nombre que
        // se destruía a sí mismo bajo concurrencia — con dos syncs cruzados, el
        // segundo copiaba ahí lo que acababa de escribir el primero, y el
        // respaldo pasaba a ser el estado intermedio de otro proceso en vez del
        // previo del usuario.
        let baks: Vec<_> = fs::read_dir(&nexum_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("settings.bak."))
            })
            .collect();
        assert_eq!(baks.len(), 1, "esperaba un backup con sello, hay {baks:?}");
        assert_eq!(fs::read_to_string(&baks[0]).unwrap(), "old");
    }

    #[test]
    fn test_write_sync_items_all_categories() {
        let home = TempDir::new().expect("创建临时 home");
        let cwd = TempDir::new().expect("创建临时 cwd");
        let home_p = home.path();
        let cwd_p = cwd.path();

        let items = SyncItems {
            settings: Some(SettingsItem {
                content: r#"{"model":"sonnet"}"#.into(),
                claude_content: None,
            }),
            skills: Some(FilesItem {
                files: vec![FileEntry {
                    path: "test-skill/SKILL.md".into(),
                    content: b"# Test Skill".to_vec(),
                }],
            }),
            mcp: Some(McpItem {
                global: Some(r#"{"global":true}"#.into()),
                project: Some(r#"{"project":true}"#.into()),
            }),
            plugins: Some(FilesItem {
                files: vec![FileEntry {
                    path: "my-plugin/manifest.json".into(),
                    content: b"{}".to_vec(),
                }],
            }),
        };

        writer::write_sync_items(home_p, cwd_p, &items).expect("全部写入应成功");

        // settings
        assert_eq!(
            fs::read_to_string(home_p.join(".peri/settings.json")).unwrap(),
            r#"{"model":"sonnet"}"#
        );
        // skills
        assert_eq!(
            fs::read_to_string(home_p.join(".claude/skills/test-skill/SKILL.md")).unwrap(),
            "# Test Skill"
        );
        // MCP global
        assert_eq!(
            fs::read_to_string(home_p.join(".mcp.json")).unwrap(),
            r#"{"global":true}"#
        );
        // MCP project
        assert_eq!(
            fs::read_to_string(cwd_p.join(".mcp.json")).unwrap(),
            r#"{"project":true}"#
        );
        // plugins
        assert_eq!(
            fs::read_to_string(home_p.join(".claude/plugins/cache/my-plugin/manifest.json"))
                .unwrap(),
            "{}"
        );
    }
}

// ─── Bloque 3: transaccionalidad ────────────────────────────────────────────
//
// Estos tests son el criterio de cierre. No alcanza con que compile: hay que
// inducir la interrupción y el cruce, que son los dos casos que el diseño
// viejo no cubría.

mod transaccional {
    use super::*;
    use crate::sync::protocol::{FilesItem, McpItem, SettingsItem, SyncItems};
    use crate::sync::writer::{self, WriteError};
    use std::fs;
    use tempfile::TempDir;

    pub(super) fn items_completos() -> SyncItems {
        SyncItems {
            settings: Some(SettingsItem {
                content: "{\"nuevo\":true}".into(),
                claude_content: None,
            }),
            mcp: Some(McpItem {
                global: Some("{\"mcp\":\"nuevo\"}".into()),
                project: None,
            }),
            skills: None,
            plugins: None,
        }
    }

    /// FALLA CERRADA: si una ruta no valida, NO se publica NADA — ni siquiera
    /// las que sí eran válidas. Antes settings se escribía y después reventaba
    /// en skills, dejando configuración mezclada sin marca ni rollback.
    #[test]
    fn una_ruta_invalida_no_deja_publicar_nada() {
        let home = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        let settings = home.path().join(".peri").join("settings.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(&settings, "{\"viejo\":true}").unwrap();

        let mut items = items_completos();
        // Un skill con path traversal: la planificación tiene que abortar.
        items.skills = Some(FilesItem {
            files: vec![crate::sync::protocol::FileEntry {
                path: "../../evasion.txt".into(),
                content: b"x".to_vec(),
            }],
        });

        let r = writer::write_sync_items(home.path(), cwd.path(), &items);
        assert!(matches!(r, Err(WriteError::PathTraversal(_))), "{r:?}");
        assert_eq!(
            fs::read_to_string(&settings).unwrap(),
            "{\"viejo\":true}",
            "settings NO puede haber cambiado: el conjunto no validó"
        );
        assert!(
            !home.path().join(".mcp.json").exists(),
            "el .mcp.json tampoco puede haberse creado"
        );
    }

    /// Dos syncs concurrentes: el segundo FALLA en vez de mezclarse.
    #[test]
    fn un_sync_concurrente_no_se_mezcla_con_el_otro() {
        let home = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        let lock = home.path().join(".peri").join("sync.lock");
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        // PID de un proceso VIVO: el nuestro.
        fs::write(&lock, std::process::id().to_string()).unwrap();

        let r = writer::write_sync_items(home.path(), cwd.path(), &items_completos());
        assert!(
            matches!(r, Err(WriteError::Concurrente(_))),
            "con otro sync en curso tiene que fallar, salió {r:?}"
        );
        assert!(
            !home.path().join(".mcp.json").exists(),
            "no puede haber escrito nada mientras el otro corre"
        );
        fs::remove_file(&lock).ok();
    }

    /// Un lock de proceso MUERTO no puede bloquear para siempre.
    #[test]
    fn un_lock_huerfano_no_bloquea() {
        let home = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        let lock = home.path().join(".peri").join("sync.lock");
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(&lock, "999999999").unwrap(); // PID que no existe

        writer::write_sync_items(home.path(), cwd.path(), &items_completos())
            .expect("un lock huérfano se limpia y el sync procede");
        assert!(home.path().join(".mcp.json").exists());
    }

    /// El backup lleva TIMESTAMP: dos syncs no pisan el respaldo del otro.
    #[test]
    fn el_backup_no_tiene_nombre_fijo() {
        let home = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        let settings = home.path().join(".peri").join("settings.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(&settings, "{\"original\":true}").unwrap();

        writer::write_sync_items(home.path(), cwd.path(), &items_completos()).unwrap();

        let baks: Vec<_> = fs::read_dir(settings.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("bak"))
            .collect();
        assert_eq!(baks.len(), 1, "esperaba un backup, hay {baks:?}");
        assert_ne!(
            baks[0], "settings.json.bak",
            "el nombre fijo es el que se destruye a sí mismo bajo concurrencia"
        );
        assert!(
            baks[0].starts_with("settings.bak."),
            "el backup tiene que llevar sello: {}",
            baks[0]
        );
    }

    /// El staging no queda de basura, ni cuando sale bien ni cuando falla.
    #[test]
    fn el_staging_se_limpia_siempre() {
        let home = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        writer::write_sync_items(home.path(), cwd.path(), &items_completos()).unwrap();
        let restos: Vec<_> = fs::read_dir(home.path().join(".peri"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("sync-staging"))
            .collect();
        assert!(restos.is_empty(), "quedó staging sin limpiar: {restos:?}");
    }

    /// El caso feliz sigue funcionando: los archivos llegan con su contenido.
    #[test]
    fn el_conjunto_valido_se_publica_entero() {
        let home = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        writer::write_sync_items(home.path(), cwd.path(), &items_completos()).unwrap();
        assert_eq!(
            fs::read_to_string(home.path().join(".peri").join("settings.json")).unwrap(),
            "{\"nuevo\":true}"
        );
        assert_eq!(
            fs::read_to_string(home.path().join(".mcp.json")).unwrap(),
            "{\"mcp\":\"nuevo\"}"
        );
    }
}

// ─── Interrupción REAL, con un proceso que se mata a mitad ──────────────────

mod interrupcion_real {
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    /// Mata un proceso DURANTE el sync y verifica que no quedó estado a medias.
    ///
    /// No es una interrupción simulada con un error inyectado: es SIGKILL sobre
    /// un proceso real que está escribiendo. Es la única forma de comprobar que
    /// las guardas por Drop no son la única red — con SIGKILL no corren.
    #[cfg(unix)]
    #[test]
    fn sigkill_durante_el_sync_no_deja_configuracion_mezclada() {
        let home = TempDir::new().unwrap();
        let peri = home.path().join(".peri");
        fs::create_dir_all(&peri).unwrap();
        fs::write(peri.join("settings.json"), "{\"viejo\":true}").unwrap();
        fs::write(home.path().join(".mcp.json"), "{\"mcp\":\"viejo\"}").unwrap();

        // Un proceso que toma el lock y se cuelga: simula el sync interrumpido
        // justo después de tomarlo y antes de publicar.
        let lock = peri.join("sync.lock");
        let hijo = Command::new("sh")
            .arg("-c")
            .arg(format!("echo $$ > {} && sleep 30", lock.display()))
            .spawn()
            .expect("spawn");
        // Esperar a que el lock exista.
        for _ in 0..50 {
            if lock.exists() && !fs::read_to_string(&lock).unwrap_or_default().trim().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // Con el lock tomado por un proceso VIVO, otro sync no puede tocar nada.
        let items = super::transaccional::items_completos();
        let r = crate::sync::writer::write_sync_items(home.path(), home.path(), &items);
        assert!(r.is_err(), "con el lock tomado tiene que rechazar: {r:?}");
        assert_eq!(
            fs::read_to_string(peri.join("settings.json")).unwrap(),
            "{\"viejo\":true}",
            "nada pudo cambiar mientras el otro tenía el lock"
        );

        // SIGKILL: las guardas por Drop NO corren. El lock queda huérfano.
        let pid = hijo.id();
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        let mut hijo = hijo;
        let _ = hijo.wait();
        // El shell dejó el lock con SU pid; puede diferir del de sh. Se fuerza
        // un pid muerto para que la detección de huérfano sea determinista.
        fs::write(&lock, "999999999").unwrap();

        // El sync siguiente TIENE que poder proceder: un lock de un muerto no
        // puede dejar la configuración congelada para siempre.
        crate::sync::writer::write_sync_items(home.path(), home.path(), &items)
            .expect("tras un lock huérfano el sync procede");
        assert_eq!(
            fs::read_to_string(peri.join("settings.json")).unwrap(),
            "{\"nuevo\":true}"
        );
        assert_eq!(
            fs::read_to_string(home.path().join(".mcp.json")).unwrap(),
            "{\"mcp\":\"nuevo\"}",
            "el conjunto se publicó ENTERO, no a medias"
        );
    }
}
