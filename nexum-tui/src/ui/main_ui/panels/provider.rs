//! Render for the `/provedor` provider catalog panel (ADR-044 cierre):
//! "Tus proveedores" + "Catálogo" + API-key input modal.

use nexum_widgets::BorderedPanel;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph},
    Frame,
};

use crate::{
    app::{
        provider_panel::{
            tier_title, CatalogInput, CatalogProviderEntry, CatalogStatus, InputFocus, InputPhase,
            LoginSuccess, ProviderPanel, Row,
        },
        App,
    },
    config::ProviderConfig,
    ui::theme,
};

/// Internal left padding (columns) so content isn't flush against the border.
const PAD: usize = 3;

/// Map the probe-returned model list onto the legacy opus/sonnet/haiku slots
/// (ProviderModels). First model → opus (the default active alias), etc. This
/// makes /modelo + inference robust even if the catalog JSON is regenerated,
/// and the first model is immediately usable (active_alias defaults to "opus").
fn models_to_provider_models(models: &[String]) -> crate::config::ProviderModels {
    let nth = |i: usize| -> String { models.get(i).cloned().unwrap_or_default() };
    crate::config::ProviderModels {
        opus: nth(0),
        sonnet: nth(1),
        haiku: nth(2),
    }
}

/// Persist a validated one-step login into NexumConfig so the provider is
/// immediately usable for chat (LlmProvider::from_config requires id + type +
/// apiKey + baseUrl). The raw key is written only to the settings file the
/// runtime already uses for provider credentials.
fn upsert_provider_config(app: &mut App, success: &LoginSuccess) {
    let cfg_arc = app.services.nexum_config.clone();
    let mut cfg = cfg_arc.write();
    let ptype = if success.protocol == "anthropic" {
        "anthropic"
    } else {
        "openai"
    };
    let pmodels = models_to_provider_models(&success.models);
    if let Some(p) = cfg
        .config
        .providers
        .iter_mut()
        .find(|p| p.id == success.provider_id)
    {
        p.provider_type = ptype.to_string();
        p.api_key = success.api_key.clone();
        p.base_url = success.base_url.clone();
        p.name = Some(success.display_name.clone());
        // Solo sobrescribimos los slots si el probe devolvió modelos, para no
        // pisar config manual con vacíos.
        if !pmodels.opus.is_empty() {
            p.models = pmodels;
        }
    } else {
        cfg.config.providers.push(ProviderConfig {
            id: success.provider_id.clone(),
            provider_type: ptype.to_string(),
            api_key: success.api_key.clone(),
            base_url: success.base_url.clone(),
            name: Some(success.display_name.clone()),
            models: pmodels.clone(),
            ..Default::default()
        });
    }
    if let Err(e) = App::save_config(&cfg, app.services.config_path_override.as_deref()) {
        drop(cfg);
        app.session_mgr
            .current_mut()
            .messages
            .push_system_note(format!("No se pudo guardar la config: {e}"));
        return;
    }
    drop(cfg);
    // Workspace settings ({cwd}/.peri/settings.json) REPLACE the global
    // providers list at load time (merge_overrides). If a workspace file
    // exists, patch its providers too or the new provider would vanish on the
    // next start. (Shared helper with the /modelo resolver path.)
    crate::app::model_panel::patch_workspace_provider(&crate::app::model_panel::ResolvedProvider {
        provider_id: success.provider_id.clone(),
        display_name: success.display_name.clone(),
        base_url: success.base_url.clone(),
        api_key: success.api_key.clone(),
        protocol: success.protocol.clone(),
    });
}

/// Consume a pending async login result (probe subprocess), applying it to the
/// panel state and — on success — persisting config + reloading the catalog.
fn consume_login_result(f_panel: &mut ProviderPanel, app: &mut App) {
    let Some(outcome) = f_panel.poll_login_result() else {
        return;
    };
    // Validation finished (either way): stop the forced-redraw loading state.
    app.session_mgr.current_mut().ui.loading = false;
    if let Some(success) = f_panel.apply_outcome(outcome) {
        upsert_provider_config(app, &success);
        // Fix UX popups (Bug 3): sin system note de éxito — llenaba
        // view_messages y sacaba al usuario del splash. El feedback es la
        // fila del provider apareciendo en "Tus proveedores" (reload abajo).
        // Reload: the Python side already upserted the catalog JSON, so the
        // provider jumps from Catálogo to "Tus proveedores" at once.
        f_panel.reload();
    }
}

/// Arma el mensaje de UX de fallo del OAuth con datos seguros (B4).
///
/// Muestra: provider, callback esperado (host:port path), listener detectado
/// (sí/no + IPv4/IPv6), y recomendación. NUNCA muestra code/token/state ni
/// la callback URL completa (solo componentes sanitizados).
fn format_callback_failure_hint(
    family: &str,
    msg: &str,
    diag: crate::app::provider_panel::CallbackDiag,
) -> String {
    let mut lines = vec![format!(
        "Login de {family}: el callback local no fue capturado."
    )];
    lines.push(format!("  Detalle: {msg}"));
    // Callback esperado (solo si lo conocemos).
    match (&diag.host, diag.port, &diag.path) {
        (Some(h), Some(p), Some(path)) => {
            lines.push(format!(
                "  Callback esperado: {h}:{p}{path} (origen: {})",
                diag.source
            ));
        }
        _ => {
            lines.push(
                "  Callback esperado: desconocido (la auth_url no incluía redirect_uri).".into(),
            );
        }
    }
    // Listener detectado.
    let listener_line = match diag.listener_kind.as_str() {
        "missing" => "  Listener detectado: NO (IPv4 ✗ / IPv6 ✗)".to_string(),
        "ipv4_only" => "  Listener detectado: sí (IPv4 únicamente)".to_string(),
        "ipv6_only" => "  Listener detectado: sí (IPv6 únicamente)".to_string(),
        "both" => "  Listener detectado: sí (IPv4 + IPv6)".to_string(),
        other => format!("  Listener detectado: {other}"),
    };
    lines.push(listener_line);
    // Recomendación.
    if diag.listener_kind == "missing" {
        lines.push("  Recomendación: CLIProxyAPI no está escuchando el callback de este".into());
        lines.push(
            "    provider. Reiniciá cli-proxy-api (`systemctl --user restart cli-proxy-api`)"
                .into(),
        );
        lines.push("    o verificá que el login del provider esté habilitado en su config.".into());
        lines.push(
            "  Limitación upstream: CLIProxyAPI no expone un endpoint para inyectar el".into(),
        );
        lines.push(
            "    code manualmente; el callback debe ser capturado por su listener interno.".into(),
        );
    } else {
        lines.push("  Recomendación: reintentá el login (r) o verificá la conexión de red.".into());
    }
    lines.join("\n")
}

/// Consume el resultado de un bridge job (refresh `r` / conectar puente).
fn consume_bridge_job(panel: &mut ProviderPanel, app: &mut App) {
    use crate::app::provider_panel::BridgeJobOutcome;
    let Some(outcome) = panel.poll_bridge_job() else {
        return;
    };
    app.session_mgr.current_mut().ui.loading = false;
    match outcome {
        BridgeJobOutcome::RefreshDone(Ok(())) => {
            // El catálogo ya se regeneró: recargar muestra el estado nuevo
            // (provider puenteado pasa a usable con sus modelos).
            panel.reload();
        }
        BridgeJobOutcome::ConnectDone {
            family,
            result: Ok(()),
            ..
        } => {
            // OAuth completo + catálogo regenerado: además de recargar el
            // panel, confirmación visible (spec autologin: el usuario tiene
            // que VER que quedó conectado, no inferirlo).
            panel.reload();
            app.session_mgr
                .current_mut()
                .messages
                .push_system_note(format!(
                    "{family} conectado correctamente. Abrí /modelo para elegir un modelo."
                ));
        }
        BridgeJobOutcome::RefreshDone(Err(msg)) => {
            let note = format!("Refresh del puente falló: {msg}");
            app.session_mgr
                .current_mut()
                .messages
                .push_system_note(note.clone());
            // reload() reconstruye el panel, así que el banner se publica
            // DESPUÉS o se perdería.
            panel.reload();
            panel.set_action_error(note);
        }
        BridgeJobOutcome::ConnectDone {
            family,
            result: Err(msg),
            callback_diag,
        } => {
            // Si falló y conocemos el callback, enriquecemos el mensaje con
            // diagnóstico útil (host/port/path sanitizados + listener sí/no).
            // NUNCA code/token/state (no viajan en callback_diag por diseño).
            let hint = format_callback_failure_hint(&family, &msg, callback_diag);
            app.session_mgr
                .current_mut()
                .messages
                .push_system_note(hint.clone());
            // El chat está tapado por este panel: sin esto, el fallo se percibe
            // como un no-op silencioso (causa raíz RC-8 del diagnóstico).
            panel.set_action_error(hint);
        }
    }
}

pub(crate) fn render_provider_panel(
    f: &mut Frame,
    panel: &mut ProviderPanel,
    app: &mut App,
    area: Rect,
) {
    consume_login_result(panel, app);
    consume_bridge_job(panel, app);
    // Paint a solid popup background over the whole area so terminal transparency
    // does not bleed through the panel content.
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(theme::POPUP_BG)),
        area,
    );

    let title = " Proveedores Nexum ";
    let inner = BorderedPanel::new(Span::styled(
        title,
        Style::default()
            .fg(theme::THINKING)
            .add_modifier(Modifier::BOLD),
    ))
    .border_style(Style::default().fg(theme::NEXUM_PRIMARY))
    .render(f, area);

    app.session_mgr.current_mut().ui.panel_area = Some(inner);

    let pad = " ".repeat(PAD);
    let mut lines: Vec<Line> = Vec::new();

    // ── Error / missing-catalog path ─────────────────────────────────────────
    if let Some(err) = panel.error.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("{pad}Provider catalog no disponible."),
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        for ln in err.lines() {
            lines.push(Line::from(Span::styled(
                format!("{pad}{ln}"),
                Style::default().fg(theme::TEXT),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("{pad}No se muestran datos heredados. No hay fallback."),
            Style::default().fg(theme::MUTED),
        )));
        push_footer(&mut lines, &pad);
        render_paragraph(f, lines, inner);
        return;
    }

    // ── Banner de error de acción (connect / refresh) ────────────────────────
    // No es terminal: el catálogo se sigue mostrando debajo. Existe para que un
    // connect fallido nunca se vea como si no hubiera pasado nada.
    if let Some(err) = panel.action_error.as_deref() {
        for (idx, ln) in err.lines().enumerate() {
            let style = if idx == 0 {
                Style::default()
                    .fg(theme::ERROR)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT)
            };
            lines.push(Line::from(Span::styled(format!("{pad}{ln}"), style)));
        }
        lines.push(Line::from(""));
    }

    // ── Status line ──────────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        format!("{pad}Catálogo válido · fuente resuelta sin checkout"),
        Style::default().fg(theme::MUTED),
    )));
    if let Some(job) = &panel.bridge_job {
        let secs = job.started.elapsed().as_secs();
        lines.push(Line::from(Span::styled(
            format!("{pad}⟳ {} ({secs}s · Esc cancela)", job.label),
            Style::default().fg(theme::ACCENT),
        )));
    }
    lines.push(Line::from(""));

    let rows = panel.rows();
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                "{pad}Catálogo vacío. Ejecutá:\n\
                 {pad}  nexum provider reconcile"
            ),
            Style::default().fg(theme::MUTED),
        )));
        push_footer(&mut lines, &pad);
        render_paragraph(f, lines, inner);
        return;
    }

    let selected = panel.selected();
    let expanded = panel.expanded_id().map(|s| s.to_string());
    let cli_proxy = panel.catalog.as_ref().and_then(|d| d.cli_proxy_api.clone());

    // Rango de líneas de la fila seleccionada (para el viewport, Bug 1).
    let mut sel_start: usize = 0;
    let mut sel_end: usize = 0;

    let rows = rows.to_vec();
    for (i, row) in rows.iter().enumerate() {
        if i == selected {
            sel_start = lines.len();
        }
        match row {
            Row::Header { tier } => {
                if i > 0 {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(
                    format!("{pad}{}", tier_title(*tier)),
                    header_style(*tier),
                )));
            }
            Row::Provider { entry, .. } => {
                let status = CatalogStatus::from_str(&entry.status);
                let is_selected = i == selected;
                let is_expanded = expanded.as_deref() == Some(entry.id.as_str());
                render_provider_row(
                    &mut lines,
                    entry,
                    status,
                    is_selected,
                    is_expanded,
                    &pad,
                    cli_proxy.as_ref(),
                );
            }
            Row::CatalogEntry { entry } => {
                // Mejora 4 (Sprint B): aire entre ítems del catálogo — una
                // línea en blanco antes de cada uno salvo el primero de la
                // sección.
                let first_of_section =
                    matches!(rows.get(i.wrapping_sub(1)), Some(Row::Header { .. }));
                if !first_of_section {
                    lines.push(Line::from(""));
                }
                if i == selected {
                    sel_start = lines.len();
                }
                let is_selected = i == selected;
                let cursor = if is_selected { "❯" } else { " " };
                // Jerarquía: nombre en verde primario bold (magenta bold bajo
                // el cursor); acción atenuada; hint de key aún más atenuado.
                let name_style = if is_selected {
                    Style::default()
                        .fg(theme::THINKING)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(theme::NEXUM_PRIMARY)
                        .add_modifier(Modifier::BOLD)
                };
                let name_field = format!("{:<24}", entry.display_name);
                lines.push(Line::from(vec![
                    Span::styled(pad.clone(), Style::default()),
                    Span::styled(
                        format!("{cursor} ○ "),
                        if is_selected {
                            Style::default()
                                .fg(theme::THINKING)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme::MUTED)
                        },
                    ),
                    Span::styled(name_field, name_style),
                    Span::styled(
                        "Enter: conectar con API key".to_string(),
                        Style::default().fg(theme::MUTED),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    format!("{pad}    Key: {}", entry.key_env_hint),
                    Style::default().fg(theme::DIM),
                )));
            }
        }
        if i == selected {
            sel_end = lines.len().saturating_sub(1).max(sel_start);
        }
    }

    push_footer(&mut lines, &pad);

    // ── Viewport con scroll (Bug 1): el offset sigue a la fila seleccionada ──
    let total = lines.len();
    let viewport = inner.height as usize;
    // Margen de contexto (scrolloff): con la primera fila seleccionada el
    // offset vuelve a 0 (status line + header visibles, sin ▲ espurio).
    let offset = super::scroll::ensure_visible(
        panel.scroll_offset(),
        sel_start.saturating_sub(3),
        sel_end,
        viewport,
        total,
    );
    panel.set_viewport_offset(offset);
    let content_total = super::model::content_line_count(&lines);
    // Sprint B, Bug 3: con el modal abierto la lista de fondo se atenúa
    // (todo a DIM, cursor ❯ incluido) para que quede claro que el foco es
    // exclusivo del modal. Las teclas ya las consume el modal desde el
    // cierre ADR-044; esto agrega el feedback visual que faltaba.
    let modal_open = panel.input.is_some();
    let visible: Vec<Line> = lines
        .into_iter()
        .skip(offset)
        .take(viewport)
        .map(|l| if modal_open { dim_line(l) } else { l })
        .collect();
    render_paragraph(f, visible, inner);
    if !modal_open {
        super::model::render_overflow_indicators(f, inner, offset, viewport, content_total);
    }

    // ── API-key input modal (overlay, on top of the panel) ──────────────────
    if let Some(input) = panel.input.clone() {
        render_input_modal(f, &input, inner);
    }
}

/// Atenúa una línea completa a `theme::DIM` (fondo intacto). Usado para la
/// lista de fondo mientras el modal de conexión está abierto.
fn dim_line(line: Line<'_>) -> Line<'_> {
    let spans = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content, Style::default().fg(theme::DIM)))
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// Centered modal with masked API-key input (and base URL field when the
/// provider is region-specific, e.g. MiMo Token Plan).
///
/// Sprint B, Bug 2 (diagnóstico (a): ancho fijo 72 sin wrap truncaba los
/// hints largos): ancho = 65% del área disponible, clamp [60, 100] columnas,
/// nunca menor que el título; el texto interno hace wrap y la altura se
/// calcula según las líneas ya wrapeadas.
fn render_input_modal(f: &mut Frame, input: &CatalogInput, area: Rect) {
    let title = format!(" Conectar {} ", input.entry.display_name);
    // El título nunca se trunca: el ancho mínimo lo cubre (+2 de bordes).
    let title_min = title.chars().count() as u16 + 2;
    let width = ((area.width as u32 * 65 / 100) as u16)
        .clamp(60, 100)
        .max(title_min)
        .min(area.width.saturating_sub(2).max(title_min));

    let mut lines: Vec<Line> = Vec::new();
    let label = |focused: bool| {
        if focused {
            Style::default()
                .fg(theme::THINKING)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::MUTED)
        }
    };

    // Instrucción explícita primero (spec autologin: "Pegá tu API key de
    // Z.ai Coding Plan"): qué hacer, en una línea, sin mencionar OAuth ni
    // logins de apps de escritorio. El hint de dónde sacar la key va debajo.
    lines.push(Line::from(Span::styled(
        format!(" Pegá tu API key de {}.", input.entry.display_name),
        Style::default().fg(theme::TEXT),
    )));
    lines.push(Line::from(Span::styled(
        format!(" Conseguila en: {}", input.entry.key_env_hint),
        Style::default().fg(theme::MUTED),
    )));
    lines.push(Line::from(""));

    if input.entry.needs_base_url {
        let shown = if input.base_url_buf.is_empty() {
            format!("{} (pegala de tu dashboard)", input.entry.base_url)
        } else {
            input.base_url_buf.clone()
        };
        let val_style = if input.base_url_buf.is_empty() {
            Style::default().fg(theme::MUTED)
        } else {
            Style::default().fg(theme::TEXT)
        };
        lines.push(Line::from(vec![
            Span::styled(" Base URL: ", label(input.focus == InputFocus::BaseUrl)),
            Span::styled(shown, val_style),
        ]));
    }

    // Masked key: never render the raw value.
    let masked = "•".repeat(input.key_buf.chars().count().min(48));
    lines.push(Line::from(vec![
        Span::styled(" API key:  ", label(input.focus == InputFocus::Key)),
        Span::styled(masked, Style::default().fg(theme::TEXT)),
    ]));
    lines.push(Line::from(""));

    match &input.phase {
        InputPhase::Validating => {
            let secs = input
                .started
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0)
                .min(99);
            lines.push(Line::from(Span::styled(
                format!(" Validando… {secs}s (probe en vivo, timeout 5s)"),
                Style::default().fg(theme::ACCENT),
            )));
        }
        InputPhase::NetworkError(_msg) => {
            lines.push(Line::from(Span::styled(
                " No se pudo contactar al proveedor. ¿Reintentar? (r/Esc)",
                Style::default().fg(theme::ERROR),
            )));
        }
        InputPhase::Editing => {
            if let Some(err) = &input.error {
                lines.push(Line::from(Span::styled(
                    format!(" {err}"),
                    Style::default().fg(theme::ERROR),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    " Enter: validar · Esc: cancelar",
                    Style::default().fg(theme::MUTED),
                )));
            }
        }
    }

    // Altura dinámica: líneas ya contadas CON wrap al ancho interno real.
    let inner_width = width.saturating_sub(2).max(1) as usize;
    let wrapped_rows: usize = lines
        .iter()
        .map(|l| {
            let w: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            w.div_ceil(inner_width).max(1)
        })
        .sum();
    let height = ((wrapped_rows as u16) + 2).min(area.height.max(3));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal = Rect::new(x, y, width, height);

    // Fondo sólido ANTES del contenido (sin bleed-through de lo de atrás).
    f.render_widget(Clear, modal);
    f.render_widget(
        Block::default().style(Style::default().bg(theme::POPUP_BG)),
        modal,
    );
    let inner = BorderedPanel::new(Span::styled(
        title,
        Style::default()
            .fg(theme::THINKING)
            .add_modifier(Modifier::BOLD),
    ))
    .border_style(Style::default().fg(theme::NEXUM_PRIMARY))
    .render(f, modal);

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .block(Block::default().style(Style::default().bg(theme::POPUP_BG))),
        inner,
    );
}

fn render_provider_row(
    lines: &mut Vec<Line>,
    entry: &CatalogProviderEntry,
    status: CatalogStatus,
    is_selected: bool,
    is_expanded: bool,
    pad: &str,
    cli_proxy: Option<&crate::app::provider_panel::CliProxyApiInfo>,
) {
    let cursor = if is_selected { "❯" } else { " " };
    let glyph = status.glyph();

    // Mejora 4 (Sprint B) — jerarquía tipográfica: nombre en verde primario
    // bold si el provider está USABLE, atenuado si no; el cursor lo pinta
    // magenta bold (consistente con el resto de los popups).
    let usable = matches!(status, CatalogStatus::UsableNow | CatalogStatus::Connected);
    let name_style = if is_selected {
        Style::default()
            .fg(theme::THINKING)
            .add_modifier(Modifier::BOLD)
    } else if usable {
        Style::default()
            .fg(theme::NEXUM_PRIMARY)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::MUTED)
    };
    // Estado por semántica: verde OK · amarillo acción requerida · rojo error.
    let status_style = match status {
        CatalogStatus::UsableNow | CatalogStatus::Connected => {
            Style::default().fg(theme::NEXUM_PRIMARY)
        }
        CatalogStatus::DetectedLogin
        | CatalogStatus::NativeLoginDetected
        | CatalogStatus::BridgeNotInstalled
        | CatalogStatus::BridgeNotRunning
        | CatalogStatus::BridgeNotActive
        | CatalogStatus::BridgeManagementLocked
        | CatalogStatus::Expired
        | CatalogStatus::ProbePending
        | CatalogStatus::ProbeFailed
        | CatalogStatus::MimoDifferentFormat => Style::default().fg(theme::WARNING),
        CatalogStatus::Error => Style::default().fg(theme::ERROR),
        _ => Style::default().fg(theme::MUTED),
    };

    // Primary row: pad + cursor + glyph + name (padded) + status label.
    let name_field = format!("{:<24}", entry.display_name);
    lines.push(Line::from(vec![
        Span::styled(pad.to_string(), Style::default()),
        Span::styled(
            format!("{cursor} {glyph} "),
            if is_selected {
                Style::default()
                    .fg(theme::THINKING)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::MUTED)
            },
        ),
        Span::styled(name_field, name_style),
        Span::styled(status.label().to_string(), status_style),
    ]));

    // Inline summary: show next_action (v2) or a compact status hint.
    // ADR-044 rule: never mute — every non-usable provider shows a clear next_action.
    let inline: Option<String> = if entry.is_usable() {
        if entry.id == "ollama" || entry.id == "ollama_local" {
            let uf = entry.model_policy.user_facing_count.unwrap_or(0).max(0);
            let rs = entry
                .model_policy
                .reserved_internal_count
                .unwrap_or(0)
                .max(0);
            Some(format!("User-facing: {uf} · Reservados: {rs}"))
        } else if entry.credential_detected {
            Some("Credencial detectada · valor protegido".to_string())
        } else {
            None
        }
    } else if let Some(detail) = entry.credential_detail.as_deref() {
        // El catálogo ya sabe POR QUÉ este provider no sirve, con el motivo
        // textual del propio proveedor. Mostrar "→ connect" en su lugar era
        // mentir por omisión: no le falta login, le falta crédito.
        let origen = entry
            .credential_store
            .as_deref()
            .map(|s| format!(" · {s}"))
            .unwrap_or_default();
        Some(format!("{detail}{origen}"))
    } else {
        // Non-usable: prefer next_action (v2), fall back to bridge_detail / status_detail.
        entry
            .next_action
            .clone()
            .or_else(|| entry.bridge_detail.clone())
            .or_else(|| {
                if entry.status_detail.is_empty() {
                    None
                } else {
                    Some(entry.status_detail.clone())
                }
            })
    };
    if let Some(summary) = inline {
        if !summary.is_empty() {
            // next_action/hint en línea aparte, atenuado, formato consistente.
            lines.push(Line::from(Span::styled(
                format!("{pad}    → {summary}"),
                Style::default().fg(theme::DIM),
            )));
        }
    }

    // Expanded detail block (toggled by Enter).
    if is_expanded {
        push_detail(lines, entry, status, pad, cli_proxy);
    }
}

#[allow(clippy::too_many_lines)]
fn push_detail(
    lines: &mut Vec<Line>,
    entry: &CatalogProviderEntry,
    status: CatalogStatus,
    pad: &str,
    cli_proxy: Option<&crate::app::provider_panel::CliProxyApiInfo>,
) {
    let key_style = Style::default().fg(theme::MUTED);
    let val_style = Style::default().fg(theme::TEXT);
    let kv = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{pad}      {k}: "), key_style),
            Span::styled(v, val_style),
        ])
    };

    lines.push(Line::from(Span::styled(
        format!("{pad}    ┌─ detalle ──────────────"),
        Style::default().fg(theme::BORDER),
    )));
    lines.push(kv("Estado", status.label().to_string()));
    if let Some(family) = &entry.family {
        lines.push(kv("Familia", family.clone()));
    }
    if let Some(auth_mode) = &entry.auth_mode {
        lines.push(kv("Auth", auth_mode.clone()));
    }

    if let Some(base) = &entry.base_url_detected {
        lines.push(kv("Base URL", base.clone()));
    }
    if entry.credential_detected {
        lines.push(kv("Credencial", "detectada · valor protegido".to_string()));
    }
    if let Some(email) = &entry.email {
        lines.push(kv("Email", email.clone()));
    }

    // Ollama: user-facing + reserved model counts.
    if entry.id == "ollama" || entry.id == "ollama_local" {
        let uf_names: Vec<&str> = entry
            .model_policy
            .user_facing_models
            .iter()
            .take(6)
            .map(String::as_str)
            .collect();
        let rs = entry
            .model_policy
            .reserved_internal_count
            .unwrap_or(0)
            .max(0);
        if !uf_names.is_empty() {
            lines.push(kv("User-facing", uf_names.join(", ")));
        }
        if rs > 0 {
            lines.push(kv(
                "Reservados",
                format!("{rs} modelo(s) Hormiguero (no seleccionable)"),
            ));
        }
    } else if !entry.models.is_empty() {
        // v2 models list for usable non-Ollama providers.
        let names: Vec<&str> = entry.models.iter().take(6).map(String::as_str).collect();
        lines.push(kv("Modelos", names.join(", ")));
    }

    // Bridge-specific detail (v2).
    if let Some(bd) = &entry.bridge_detail {
        if !bd.is_empty() {
            lines.push(kv("Puente", bd.clone()));
        }
    }

    // Status-specific guidance + next_action.
    match status {
        CatalogStatus::DetectedLogin | CatalogStatus::NativeLoginDetected => {
            lines.push(kv("Uso directo", "no disponible todavía".to_string()));
            lines.push(kv("Requiere", "activar puente CLIProxyAPI".to_string()));
            lines.push(kv("API key", "no se solicita (login OAuth)".to_string()));
        }
        CatalogStatus::BridgeNotInstalled => {
            lines.push(kv(
                "Acción",
                "instalar CLIProxyAPI: paru -S cli-proxy-api-bin".to_string(),
            ));
        }
        CatalogStatus::BridgeNotRunning => {
            lines.push(kv(
                "Acción",
                "systemctl --user start cli-proxy-api".to_string(),
            ));
        }
        CatalogStatus::BridgeNotActive => {
            lines.push(kv("Acción", "conectar puente desde /proveedor".to_string()));
        }
        CatalogStatus::BridgeManagementLocked => {
            lines.push(kv(
                "Acción",
                "configurar CLIPROXYAPI_MANAGEMENT_KEY".to_string(),
            ));
        }
        CatalogStatus::Expired => {
            lines.push(kv("Acción", "reconectar puente".to_string()));
        }
        CatalogStatus::RequiresApiKey => {
            lines.push(kv("Acción", "conectar con API key (futuro)".to_string()));
        }
        CatalogStatus::Connected => {
            lines.push(kv("Modelos", "probe online pendiente".to_string()));
        }
        CatalogStatus::MimoDifferentFormat => {
            lines.push(kv(
                "Acción",
                "adapter MiMo específico requerido".to_string(),
            ));
        }
        _ => {}
    }
    // ── Sprint C §5.4: estado del puente en 3 líneas (solo providers
    // puenteados por CLIProxyAPI), con semántica de color del Sprint B.
    if ProviderPanel::is_bridge_provider(entry) {
        let verde = Style::default().fg(theme::NEXUM_PRIMARY);
        let amarillo = Style::default().fg(theme::WARNING);
        let rojo = Style::default().fg(theme::ERROR);
        let (cpa_label, cpa_style) = match cli_proxy {
            Some(i) if i.running => ("running", verde),
            Some(i) if i.installed => ("not_running", amarillo),
            Some(_) => ("not_installed", rojo),
            None => ("desconocido", Style::default().fg(theme::MUTED)),
        };
        let (mgmt_label, mgmt_style) = match cli_proxy {
            Some(i) if i.status.as_deref() == Some("bridge_ok") => ("available", verde),
            Some(i) if i.status.as_deref() == Some("bridge_management_locked") => {
                ("locked", amarillo)
            }
            Some(i) if i.running => ("unavailable", rojo),
            _ => ("unavailable", Style::default().fg(theme::MUTED)),
        };
        let (puente_label, puente_style) = match entry.bridge_status.as_deref() {
            Some("usable") => ("usable", verde),
            Some("expired") => ("expirado", rojo),
            _ => ("no_activado", amarillo),
        };
        let kv_status = |k: &str, v: &str, st: Style| {
            Line::from(vec![
                Span::styled(format!("{pad}      {k}: "), key_style),
                Span::styled(v.to_string(), st),
            ])
        };
        lines.push(kv_status("CLIProxyAPI", cpa_label, cpa_style));
        lines.push(kv_status("Management", mgmt_label, mgmt_style));
        lines.push(kv_status("Puente", puente_label, puente_style));
    }

    // Always show next_action if present (v2: never mute).
    if let Some(na) = &entry.next_action {
        if !na.is_empty() {
            lines.push(kv("Next", na.clone()));
        }
    }
    lines.push(Line::from(""));
}

fn header_style(tier: u8) -> Style {
    match tier {
        0 => Style::default()
            .fg(theme::NEXUM_PRIMARY)
            .add_modifier(Modifier::BOLD),
        1 | 4 => Style::default()
            .fg(theme::THINKING)
            .add_modifier(Modifier::BOLD),
        2 => Style::default()
            .fg(theme::MUTED)
            .add_modifier(Modifier::BOLD),
        3 => Style::default()
            .fg(theme::ERROR)
            .add_modifier(Modifier::BOLD),
        _ => Style::default(),
    }
}

fn push_footer(lines: &mut Vec<Line>, pad: &str) {
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("{pad}↑↓/jk :Mover   Tab :Sección   Enter :Detalles   Esc :Cerrar"),
        Style::default().fg(theme::MUTED),
    )));
}

fn render_paragraph(f: &mut Frame, lines: Vec<Line>, area: Rect) {
    let block = Block::default().style(Style::default().bg(theme::POPUP_BG));
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}
