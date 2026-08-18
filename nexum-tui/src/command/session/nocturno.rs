//! /nocturno — superficie del ciclo de aprendizaje controlado (OMEGA 6b).
//!
//! Invoca el CLI `python3 -m nexum_nocturno <subcmd>` con timeout duro (3s).
//! Es un comando MANUAL (no hot path): el bloqueo breve es aceptable, igual
//! que /hormiguero status. Nunca muestra contenido privado: el CLI ya emite
//! solo códigos/estados/métricas.

use crate::{app::App, command::Command};

pub struct NocturnoCommand;

const ALLOWED: &[&str] = &[
    "status",
    "candidates",
    "inspect",
    "approve",
    "reject",
    "rollback",
    "history",
];

impl Command for NocturnoCommand {
    fn name(&self) -> &str {
        "nocturno"
    }

    fn description(&self, _lc: &crate::i18n::LcRegistry) -> String {
        "Nocturno: aprendizaje controlado (status | candidates | inspect | approve | reject | rollback | history)".to_string()
    }

    fn execute(&self, app: &mut App, args: &str) {
        let mut parts = args.trim().split_whitespace();
        let sub = parts.next().unwrap_or("status");
        if !ALLOWED.contains(&sub) {
            app.push_system_note(format!(
                "/nocturno: subcomando desconocido '{sub}'. Usá: {}",
                ALLOWED.join(" | ")
            ));
            app.render_rebuild();
            return;
        }
        // El id (si viene) se sanea: solo [a-zA-Z0-9-] (UUIDs), nada de shell.
        let arg1: String = parts
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if matches!(sub, "inspect" | "approve" | "reject" | "rollback") && arg1.is_empty() {
            app.push_system_note(format!("/nocturno {sub} <candidate_id> — falta el id."));
            app.render_rebuild();
            return;
        }
        let note = match run_cli(sub, &arg1) {
            Ok(json) => format_note(sub, &json),
            Err(e) => format!("/nocturno {sub}: no disponible ({e})"),
        };
        app.push_system_note(note);
        app.render_rebuild();
    }
}

/// Ejecuta el CLI python con timeout duro. Devuelve el stdout (JSON).
fn run_cli(sub: &str, arg1: &str) -> Result<String, String> {
    let pythonpath = find_pythonpath_dir().ok_or("no encontré los sidecars de Nexum")?;
    let mut cmd = std::process::Command::new("python3");
    cmd.args(["-m", "nexum_nocturno", sub]);
    if !arg1.is_empty() {
        cmd.arg(arg1);
    }
    cmd.env("PYTHONPATH", &pythonpath)
        .current_dir(&pythonpath)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timeout 3s".into());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
    let mut out = String::new();
    use std::io::Read as _;
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    if out.trim().is_empty() {
        return Err("sin salida".into());
    }
    Ok(out)
}

/// Render seguro del JSON del CLI (claves conocidas; nunca contenido libre).
fn format_note(sub: &str, json: &str) -> String {
    let parsed: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return format!("/nocturno {sub}: respuesta inválida del CLI"),
    };
    if parsed.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let err = parsed
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("error");
        return format!("/nocturno {sub}: {err}");
    }
    match sub {
        "status" => format!(
            "/nocturno status\n⎿ modo: {} · autopromote: {}\n⎿ policy: {}\n⎿ candidatos: {}\n⎿ evidencia: {} registros, cadena {}",
            parsed["mode"].as_str().unwrap_or("-"),
            if parsed["autopromote"].as_bool().unwrap_or(false) { "ON" } else { "OFF" },
            parsed["policy_version"].as_str().unwrap_or("-"),
            serde_json::to_string(&parsed["candidates_by_state"]).unwrap_or_default(),
            parsed["evidence_records"].as_u64().unwrap_or(0),
            if parsed["evidence_chain_ok"].as_bool().unwrap_or(false) { "OK" } else { "ROTA" },
        ),
        "candidates" | "history" => {
            let empty = vec![];
            let cands = parsed["candidates"].as_array().unwrap_or(&empty);
            let mut lines = vec![format!("/nocturno {sub} — {} candidato(s)", cands.len())];
            for c in cands.iter().take(10) {
                lines.push(format!(
                    "⎿ {} · {} · {} · {}",
                    c["candidate_id"].as_str().unwrap_or("-").chars().take(8).collect::<String>(),
                    c["kind"].as_str().unwrap_or("-"),
                    c["state"].as_str().unwrap_or("-"),
                    c["hypothesis_code"].as_str().unwrap_or("-"),
                ));
            }
            lines.join("\n")
        }
        _ => {
            let c = &parsed["candidate"];
            format!(
                "/nocturno {sub}\n⎿ id: {}\n⎿ kind: {} · estado: {}\n⎿ métricas: base {} → cand {}\n⎿ motivo rechazo: {}",
                c["candidate_id"].as_str().unwrap_or("-"),
                c["kind"].as_str().unwrap_or("-"),
                c["state"].as_str().unwrap_or("-"),
                serde_json::to_string(&c["baseline_metrics"]).unwrap_or_default(),
                serde_json::to_string(&c["candidate_metrics"]).unwrap_or_default(),
                c["reject_reason"].as_str().unwrap_or("-"),
            )
        }
    }
}

/// Devuelve el dir que debe ir en PYTHONPATH (contiene el paquete
/// `nexum_nocturno`). Prioridad: NEXUM_CLI_DIR (checkout, `src/`) > layout del
/// artefacto instalado (`<slot>/src/`) > ancestros del binario (checkout
/// `src/`). NO depende del checkout: el artefacto instalado descubre sus
/// sidecars desde el slot instalado (`InstalledLayoutV1`).
fn find_pythonpath_dir() -> Option<std::path::PathBuf> {
    let has_pkg = |root: &std::path::Path| root.join("nexum_nocturno").is_dir();

    if let Ok(d) = std::env::var("NEXUM_CLI_DIR") {
        let src = std::path::PathBuf::from(d).join("src");
        if has_pkg(&src) {
            return Some(src);
        }
    }
    if let Some(layout) = crate::layout::InstalledLayoutV1::current() {
        let src = layout.version_root().join("src");
        if has_pkg(&src) {
            return Some(src);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut a = exe.as_path();
        for _ in 0..6 {
            let parent = a.parent()?;
            a = parent;
            let src = a.join("src");
            if has_pkg(&src) {
                return Some(src);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_nocturno_subcomando_desconocido() {
        let (mut app, _) = crate::app::App::new_headless(80, 24).await;
        NocturnoCommand.execute(&mut app, "hackear");
        let last = app
            .session_mgr
            .current()
            .messages
            .view_messages
            .last()
            .cloned();
        if let Some(crate::ui::message_view::MessageViewModel::SystemNote { content, .. }) = last {
            assert!(content.contains("desconocido"), "{content}");
        } else {
            panic!("esperaba SystemNote");
        }
    }

    #[tokio::test]
    async fn test_nocturno_approve_sin_id_pide_id() {
        let (mut app, _) = crate::app::App::new_headless(80, 24).await;
        NocturnoCommand.execute(&mut app, "approve");
        let last = app
            .session_mgr
            .current()
            .messages
            .view_messages
            .last()
            .cloned();
        if let Some(crate::ui::message_view::MessageViewModel::SystemNote { content, .. }) = last {
            assert!(content.contains("falta el id"), "{content}");
        } else {
            panic!("esperaba SystemNote");
        }
    }

    #[test]
    fn test_sanea_id_contra_shell_injection() {
        // El filtro de chars solo deja [a-zA-Z0-9-]: nada de ; | $ ( ) etc.
        let dirty = "abc; rm -rf / $(evil)";
        let clean: String = dirty
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        assert_eq!(clean, "abcrm-rfevil");
    }
}
