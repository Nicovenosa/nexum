//! /memoria — memoria honesta mínima (SPEC-MEMORY-001, M-3).
//!
//! Flujo de escritura SIEMPRE: propuesta visible → confirmación explícita
//! → MemoryGateway. Nunca escrituras silenciosas; un modelo jamás escribe
//! directo. Con backend caído: degradación explícita, jamás inventar
//! recuerdos. Nunca se muestra token ni rutas de secrets.

use crate::{
    app::App,
    command::Command,
    memory_gateway::{self, client, MemoryError, PendingProposal},
};

pub struct MemoriaCommand;

impl Command for MemoriaCommand {
    fn name(&self) -> &str {
        "memoria"
    }

    fn description(&self, _lc: &crate::i18n::LcRegistry) -> String {
        "Memoria honesta mínima (status | guardar | confirmar | cancelar | buscar | listar | mostrar | borrar | conflictos | resolver | reset | on | off)".to_string()
    }

    fn execute(&self, app: &mut App, args: &str) {
        let args = args.trim();
        let (sub, resto) = match args.split_once(char::is_whitespace) {
            Some((s, r)) => (s, r.trim()),
            None => (args, ""),
        };
        match sub {
            "status" | "" => cmd_status(app),
            "guardar" => cmd_guardar(app, resto),
            "confirmar" => cmd_confirmar(app),
            "cancelar" => cmd_cancelar(app),
            "buscar" => cmd_buscar(app, resto),
            "listar" => cmd_listar(app, resto),
            "mostrar" => cmd_mostrar(app, resto),
            "borrar" => cmd_borrar(app, resto),
            "conflictos" => cmd_conflictos(app),
            "resolver" => cmd_resolver(app, resto),
            "reset" => cmd_reset(app, resto),
            "on" => cmd_toggle(app, true),
            "off" => cmd_toggle(app, false),
            other => app.push_system_note(format!(
                "/memoria: subcomando desconocido '{other}'. Usá: status | guardar <texto> | \
                 confirmar | cancelar | buscar <query> | listar | mostrar <id> | borrar <id> | \
                 conflictos | resolver <keep_both|id> [nota] | reset | on | off"
            )),
        }
        app.render_rebuild();
    }
}

fn gate_enabled(app: &mut App) -> bool {
    if !memory_gateway::env_flag_on() {
        app.push_system_note(
            "Memoria desactivada (flag NEXUM_MEMORY off — default de v0.1). \
             Para activarla: relanzá con NEXUM_MEMORY=on nexum. Sin el flag hay \
             cero lecturas y cero escrituras."
                .to_string(),
        );
        return false;
    }
    if !memory_gateway::enabled(&app.global_ui.memory_gw) {
        app.push_system_note(
            "Memoria desactivada para esta sesión (/memoria off). Reactivá con /memoria on."
                .to_string(),
        );
        return false;
    }
    true
}

fn nota_error(app: &mut App, e: &MemoryError) {
    app.push_system_note(format!("/memoria: {}", e.user_message()));
}

fn fecha_hoy() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Scope desde el prefijo `-p` (project) o user por defecto.
fn scope_de_args(resto: &str) -> (String, String, String) {
    if let Some(r) = resto.strip_prefix("-p ") {
        (
            "project".into(),
            memory_gateway::project_scope_id(),
            r.trim().to_string(),
        )
    } else {
        (
            "user".into(),
            memory_gateway::USER_SCOPE_ID.into(),
            resto.to_string(),
        )
    }
}

fn resumen_entrada(e: &crate::memory_gateway::EntryDto) -> String {
    let id_corto = &e.id[..8.min(e.id.len())];
    format!(
        "  ⎿ [{id_corto}] «{}» (scope {}:{} · estado {} · fuente {} · {})",
        e.content, e.scope_type, e.scope_id, e.status, e.source_reference, e.source_type
    )
}

fn cmd_status(app: &mut App) {
    if !memory_gateway::env_flag_on() {
        app.push_system_note(
            "Memoria (MemoryGateway)\n\
             ⎿ flag NEXUM_MEMORY: off (default v0.1 — pendiente de decisión de activación)\n\
             ⎿ efecto: cero lecturas, cero escrituras, backend no requerido\n\
             ⎿ activar: NEXUM_MEMORY=on nexum"
                .to_string(),
        );
        return;
    }
    let sesion = if memory_gateway::enabled(&app.global_ui.memory_gw) {
        "activa"
    } else {
        "desactivada por el usuario (/memoria on para reactivar)"
    };
    match client::health() {
        Ok(h) => {
            let mut extra = String::new();
            if let Ok(st) = client::status() {
                let s = &st.stats;
                extra = format!(
                    "\n ⎿ entradas: {} · schema v{} · delete: {}",
                    s.get("entries").and_then(|v| v.as_i64()).unwrap_or(0),
                    s.get("schema_version").and_then(|v| v.as_i64()).unwrap_or(0),
                    s.get("delete_mode").and_then(|v| v.as_str()).unwrap_or("-"),
                );
                let c = &st.counters;
                extra.push_str(&format!(
                    "\n ⎿ contadores: {} saves · {} rechazados · {} recalls · {} conflictos detectados · {} resueltos",
                    c.get("saves_confirmed").and_then(|v| v.as_i64()).unwrap_or(0),
                    c.get("saves_rejected").and_then(|v| v.as_i64()).unwrap_or(0),
                    c.get("recalls").and_then(|v| v.as_i64()).unwrap_or(0),
                    c.get("contradictions_detected").and_then(|v| v.as_i64()).unwrap_or(0),
                    c.get("contradictions_resolved").and_then(|v| v.as_i64()).unwrap_or(0),
                ));
            }
            app.push_system_note(format!(
                "Memoria (MemoryGateway)\n\
                 ⎿ sesión: {sesion}\n\
                 ⎿ sidecar: vivo (v{}) · búsqueda: {} · db: {}{}{}",
                h.version,
                h.search_backend.as_deref().unwrap_or("-"),
                h.db_state.as_deref().unwrap_or("-"),
                h.quarantined_path
                    .as_deref()
                    .map(|p| format!("\n ⎿ base en cuarentena: {p} (usá /memoria reset)"))
                    .unwrap_or_default(),
                extra,
            ));
        }
        Err(e) => app.push_system_note(format!(
            "Memoria (MemoryGateway)\n ⎿ sesión: {sesion}\n ⎿ sidecar: {}",
            e.user_message()
        )),
    }
}

/// Construye y muestra la propuesta. NO persiste (gate 2).
pub fn proponer_guardado(app: &mut App, contenido: &str, scope_type: &str, scope_id: &str) {
    let key = crate::memory_gateway::intent::derive_key(contenido);
    let proposal = PendingProposal {
        content: contenido.to_string(),
        key: key.clone(),
        scope_type: scope_type.to_string(),
        scope_id: scope_id.to_string(),
        source_reference: format!("chat {}", fecha_hoy()),
        idempotency_key: format!("tui-{}", uuid::Uuid::now_v7()),
    };
    app.push_system_note(format!(
        "📌 Propuesta de memoria (todavía NO guardada)\n\
         ⎿ contenido: «{}»\n\
         ⎿ scope: {}:{}\n\
         ⎿ proveniencia: user_explicit · {}\n\
         ⎿ acción: guardar como memoria estable\n\
         ⎿ confirmá con /memoria confirmar — o cancelá con /memoria cancelar",
        proposal.content, proposal.scope_type, proposal.scope_id, proposal.source_reference,
    ));
    app.global_ui.memory_gw.pending = Some(proposal);
}

fn cmd_guardar(app: &mut App, resto: &str) {
    if !gate_enabled(app) {
        return;
    }
    let (scope_type, scope_id, contenido) = scope_de_args(resto);
    if contenido.is_empty() {
        app.push_system_note(
            "/memoria guardar <texto>  (o «-p <texto>» para scope del proyecto)".to_string(),
        );
        return;
    }
    proponer_guardado(app, &contenido, &scope_type, &scope_id);
}

fn cmd_confirmar(app: &mut App) {
    if !gate_enabled(app) {
        return;
    }
    let Some(p) = app.global_ui.memory_gw.pending.take() else {
        app.push_system_note("/memoria confirmar: no hay propuesta pendiente.".to_string());
        return;
    };
    match client::save_confirmed(&p) {
        Ok(r) => {
            if let Some(conflict) = r.conflict {
                mostrar_conflicto(app, &conflict, &r.id);
                app.global_ui.memory_gw.pending_conflict = Some(conflict);
            } else {
                let id_corto = &r.id[..8.min(r.id.len())];
                app.push_system_note(format!(
                    "✅ Memoria guardada\n\
                     ⎿ id: {id_corto} ({})\n\
                     ⎿ scope: {}:{}\n\
                     ⎿ proveniencia: user_explicit · {}",
                    r.id, p.scope_type, p.scope_id, p.source_reference,
                ));
            }
        }
        Err(e) => {
            // La propuesta se conserva para reintentar (idempotency_key igual).
            app.global_ui.memory_gw.pending = Some(p);
            nota_error(app, &e);
        }
    }
}

fn cmd_cancelar(app: &mut App) {
    if app.global_ui.memory_gw.pending.take().is_some() {
        app.push_system_note(
            "Propuesta cancelada. No se escribió nada (cero writes).".to_string(),
        );
    } else {
        app.push_system_note("/memoria cancelar: no hay propuesta pendiente.".to_string());
    }
}

fn mostrar_conflicto(app: &mut App, c: &crate::memory_gateway::ConflictDto, nuevo_id: &str) {
    let mut lineas = String::new();
    for e in &c.entries {
        let marca = if e.id == nuevo_id { " ← nueva" } else { "" };
        lineas.push_str(&format!("{}{marca}\n", resumen_entrada(e)));
    }
    let g_corto = &c.group_id[..8.min(c.group_id.len())];
    app.push_system_note(format!(
        "⚠️ Contradicción detectada (key «{}», scope {}:{}) — NADA fue sobrescrito\n\
         Ambas versiones quedan conservadas:\n{lineas}\
         ⎿ conflicto: {g_corto} (estado open)\n\
         ⎿ resolvé con: /memoria resolver <id-ganador> [nota] · /memoria resolver keep_both [nota]\n\
         ⎿ o dejalo abierto: ambas versiones se muestran como en conflicto",
        c.key, c.scope_type, c.scope_id,
    ));
}

fn cmd_buscar(app: &mut App, resto: &str) {
    if !gate_enabled(app) {
        return;
    }
    let (scope_type, scope_id, query) = scope_de_args(resto);
    if query.is_empty() {
        app.push_system_note("/memoria buscar <query>  (o «-p <query>»)".to_string());
        return;
    }
    match client::recall(&query, &scope_type, &scope_id) {
        Ok(r) => {
            if r.results.is_empty() {
                app.push_system_note(format!(
                    "Sin resultados para «{query}» en scope {scope_type}:{scope_id} (motor {})",
                    r.engine
                ));
            } else {
                let mut out = format!(
                    "🔎 {} resultado(s) para «{query}» ({}:{} · motor {}):\n",
                    r.results.len(),
                    scope_type,
                    scope_id,
                    r.engine
                );
                for e in &r.results {
                    out.push_str(&resumen_entrada(e));
                    out.push('\n');
                }
                app.push_system_note(out.trim_end().to_string());
            }
        }
        Err(e) => nota_error(app, &e),
    }
}

fn cmd_listar(app: &mut App, resto: &str) {
    if !gate_enabled(app) {
        return;
    }
    let (scope_type, scope_id, _) = scope_de_args(resto);
    match client::list(&scope_type, &scope_id) {
        Ok(r) => {
            if r.results.is_empty() {
                app.push_system_note(format!("Memoria vacía en scope {scope_type}:{scope_id}."));
            } else {
                let mut out = format!(
                    "🗂 {} memoria(s) en {}:{}\n",
                    r.results.len(),
                    scope_type,
                    scope_id
                );
                for e in &r.results {
                    out.push_str(&resumen_entrada(e));
                    out.push('\n');
                }
                app.push_system_note(out.trim_end().to_string());
            }
        }
        Err(e) => nota_error(app, &e),
    }
}

/// Resuelve un id abreviado (prefijo) contra el listado del scope.
fn resolver_id(app: &mut App, prefijo: &str, scope_type: &str, scope_id: &str) -> Option<String> {
    match client::list(scope_type, scope_id) {
        Ok(r) => {
            let matches: Vec<_> = r
                .results
                .iter()
                .filter(|e| e.id.starts_with(prefijo))
                .collect();
            match matches.len() {
                1 => Some(matches[0].id.clone()),
                0 => {
                    app.push_system_note(format!(
                        "id «{prefijo}» no encontrado en {scope_type}:{scope_id}"
                    ));
                    None
                }
                _ => {
                    app.push_system_note(format!(
                        "id «{prefijo}» ambiguo ({} coincidencias) — usá más caracteres",
                        matches.len()
                    ));
                    None
                }
            }
        }
        Err(e) => {
            nota_error(app, &e);
            None
        }
    }
}

fn cmd_mostrar(app: &mut App, resto: &str) {
    if !gate_enabled(app) {
        return;
    }
    let (scope_type, scope_id, id) = scope_de_args(resto);
    if id.is_empty() {
        app.push_system_note("/memoria mostrar <id>".to_string());
        return;
    }
    let Some(full_id) = resolver_id(app, &id, &scope_type, &scope_id) else {
        return;
    };
    match client::get(&full_id, &scope_type, &scope_id) {
        Ok(r) => {
            let e = r.entry;
            app.push_system_note(format!(
                "📄 Memoria {}\n\
                 ⎿ contenido: «{}»\n\
                 ⎿ scope: {}:{} · estado: {} · versión: {}\n\
                 ⎿ proveniencia: {} · {}\n\
                 ⎿ creada: {} · actualizada: {}\n\
                 ⎿ checksum: {}…{}",
                e.id,
                e.content,
                e.scope_type,
                e.scope_id,
                e.status,
                e.version,
                e.source_type,
                e.source_reference,
                e.created_at,
                e.updated_at,
                &e.checksum[..8],
                e.contradiction_group
                    .as_deref()
                    .map(|g| format!("\n ⎿ conflicto: {}", &g[..8.min(g.len())]))
                    .unwrap_or_default(),
            ));
        }
        Err(e) => nota_error(app, &e),
    }
}

fn cmd_borrar(app: &mut App, resto: &str) {
    if !gate_enabled(app) {
        return;
    }
    let (scope_type, scope_id, id) = scope_de_args(resto);
    if id.is_empty() {
        app.push_system_note("/memoria borrar <id>".to_string());
        return;
    }
    let Some(full_id) = resolver_id(app, &id, &scope_type, &scope_id) else {
        return;
    };
    match client::delete(&full_id, &scope_type, &scope_id) {
        Ok(r) => app.push_system_note(if r.already_deleted {
            format!("La memoria {} ya estaba eliminada.", &full_id[..8])
        } else {
            format!(
                "🗑 Memoria {} eliminada (modo {} — auditable, no vuelve como activa).",
                &full_id[..8],
                r.mode
            )
        }),
        Err(e) => nota_error(app, &e),
    }
}

fn cmd_conflictos(app: &mut App) {
    if !gate_enabled(app) {
        return;
    }
    let mut total = 0;
    let mut out = String::from("⚖️ Conflictos abiertos:\n");
    for (st, si) in [
        ("user", memory_gateway::USER_SCOPE_ID.to_string()),
        ("project", memory_gateway::project_scope_id()),
    ] {
        if let Ok(r) = client::open_conflicts(st, &si) {
            for c in r.open_conflicts {
                total += 1;
                out.push_str(&format!(
                    " ⎿ [{}] key «{}» ({}:{}) — {} versiones\n",
                    &c.group_id[..8.min(c.group_id.len())],
                    c.key,
                    c.scope_type,
                    c.scope_id,
                    c.entries.len()
                ));
            }
        }
    }
    if total == 0 {
        app.push_system_note("Sin conflictos abiertos.".to_string());
    } else {
        out.push_str(" ⎿ resolvé con /memoria resolver <id-ganador> o keep_both");
        app.push_system_note(out);
    }
}

fn cmd_resolver(app: &mut App, resto: &str) {
    if !gate_enabled(app) {
        return;
    }
    let Some(conflict) = app.global_ui.memory_gw.pending_conflict.clone().or_else(|| {
        // Sin conflicto pendiente en sesión: tomar el único abierto si existe.
        let mut abiertos = vec![];
        for (st, si) in [
            ("user", memory_gateway::USER_SCOPE_ID.to_string()),
            ("project", memory_gateway::project_scope_id()),
        ] {
            if let Ok(r) = client::open_conflicts(st, &si) {
                abiertos.extend(r.open_conflicts);
            }
        }
        if abiertos.len() == 1 {
            abiertos.pop()
        } else {
            None
        }
    }) else {
        app.push_system_note(
            "/memoria resolver: no hay conflicto pendiente único. Mirá /memoria conflictos."
                .to_string(),
        );
        return;
    };
    let (eleccion, nota) = match resto.split_once(char::is_whitespace) {
        Some((e, n)) => (e.trim(), n.trim()),
        None => (resto, ""),
    };
    if eleccion.is_empty() {
        app.push_system_note(
            "/memoria resolver <id-ganador|keep_both> [nota] — la resolución es explícita, \
             jamás la decide un modelo."
                .to_string(),
        );
        return;
    }
    let nota_final = if nota.is_empty() {
        "resolución explícita del usuario desde la TUI"
    } else {
        nota
    };
    let resultado = if eleccion == "keep_both" || eleccion == "ambas" {
        client::resolve(
            &conflict.group_id,
            &conflict.scope_type,
            &conflict.scope_id,
            "keep_both",
            None,
            nota_final,
        )
    } else {
        let Some(winner) = conflict
            .entries
            .iter()
            .find(|e| e.id.starts_with(eleccion))
            .map(|e| e.id.clone())
        else {
            app.push_system_note(format!(
                "id «{eleccion}» no pertenece al conflicto. Versiones: {}",
                conflict
                    .entries
                    .iter()
                    .map(|e| format!("[{}] «{}»", &e.id[..8.min(e.id.len())], e.content))
                    .collect::<Vec<_>>()
                    .join(" · ")
            ));
            return;
        };
        client::resolve(
            &conflict.group_id,
            &conflict.scope_type,
            &conflict.scope_id,
            "winner",
            Some(&winner),
            nota_final,
        )
    };
    match resultado {
        Ok(r) => {
            app.global_ui.memory_gw.pending_conflict = None;
            let c = r.conflict;
            let detalle = c
                .entries
                .iter()
                .map(|e| format!(" ⎿ [{}] «{}» → {}", &e.id[..8.min(e.id.len())], e.content, e.status))
                .collect::<Vec<_>>()
                .join("\n");
            app.push_system_note(format!(
                "✅ Conflicto resuelto y registrado\n{detalle}\n ⎿ nota: {}",
                c.resolution_note.as_deref().unwrap_or("-"),
            ));
        }
        Err(e) => nota_error(app, &e),
    }
}

fn cmd_reset(app: &mut App, resto: &str) {
    if !gate_enabled(app) {
        return;
    }
    // R-4: acción destructiva-adyacente — exigir confirmación literal.
    if resto != "confirmar" {
        app.push_system_note(
            "/memoria reset crea una base NUEVA vacía tras una cuarentena (la base \
             aislada se preserva en disco). Es explícito e irreversible para la \
             sesión: confirmá con «/memoria reset confirmar»."
                .to_string(),
        );
        return;
    }
    match client::reset_after_quarantine() {
        Ok(v) => app.push_system_note(format!(
            "✅ Base nueva creada. La base aislada quedó preservada en: {}",
            v.get("quarantined_path")
                .and_then(|p| p.as_str())
                .unwrap_or("(ver /memoria status)")
        )),
        Err(e) => nota_error(app, &e),
    }
}

fn cmd_toggle(app: &mut App, on: bool) {
    if on && !memory_gateway::env_flag_on() {
        app.push_system_note(
            "El flag NEXUM_MEMORY está off (default v0.1): /memoria on no puede \
             activarla en caliente. Relanzá con NEXUM_MEMORY=on nexum."
                .to_string(),
        );
        return;
    }
    app.global_ui.memory_gw.session_enabled = Some(on);
    if !on {
        app.global_ui.memory_gw.pending = None;
        app.global_ui.memory_gw.pending_conflict = None;
    }
    app.push_system_note(if on {
        "Memoria reactivada para esta sesión.".to_string()
    } else {
        "Memoria desactivada para esta sesión: cero lecturas y cero escrituras \
         hasta /memoria on. La propuesta pendiente (si había) fue descartada."
            .to_string()
    });
}
