use nexum_widgets::BorderedPanel;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};

use super::scroll;
use crate::{
    app::{
        model_panel::{DisplayRow, ModelPanel},
        App,
    },
    ui::theme,
};

pub(crate) fn render_model_panel(f: &mut Frame, panel: &mut ModelPanel, app: &mut App, area: Rect) {
    let lc = &app.services.lc;
    let inner = BorderedPanel::new(Span::styled(
        lc.tr("model-panel-title"),
        Style::default()
            .fg(theme::THINKING)
            .add_modifier(Modifier::BOLD),
    ))
    .border_style(Style::default().fg(theme::NEXUM_PRIMARY))
    .render(f, area);
    paint_popup_background(f, area);

    app.session_mgr.current_mut().ui.panel_area = Some(inner);

    let cfg_guard = app.services.nexum_config.read();
    let active_provider_id_owned = cfg_guard.config.active_provider_id.clone();
    let active_provider = cfg_guard
        .config
        .providers
        .iter()
        .find(|p| p.id == active_provider_id_owned);
    let models_ref = active_provider.map(|p| &p.models);

    let is_ollama = active_provider
        .map(|p| {
            let base = p.base_url.to_lowercase();
            p.provider_type.to_lowercase() == "openai"
                && (base.contains("127.0.0.1:11434")
                    || base.contains("localhost:11434")
                    || base.contains("127.0.0.1:11435"))
        })
        .unwrap_or(false);

    let mut lines: Vec<Line> = Vec::new();
    // Línea de inicio de cada fila seleccionable (para el viewport, Bug 1).
    let mut line_of_row: Vec<Option<usize>> = vec![None; panel.row_count()];

    lines.push(Line::from(Span::styled(
        lc.tr("model-panel-description"),
        Style::default().fg(theme::MUTED),
    )));
    lines.push(Line::from(""));

    if let Some(error) = panel.catalog_error.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("ERROR: {error}"),
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "El selector no mostrará modelos heredados. Ejecutá `nexum doctor`.",
            Style::default().fg(theme::MUTED),
        )));
    }

    let display_rows = panel.display_rows().to_vec();
    for (row_idx, display_row) in display_rows.iter().enumerate() {
        match display_row {
            DisplayRow::Header { family, .. } => {
                if row_idx > 0 {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {} ", "\u{2714}"),
                        Style::default().fg(theme::NEXUM_PRIMARY),
                    ),
                    Span::styled(
                        family.clone(),
                        Style::default()
                            .fg(theme::THINKING)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
            DisplayRow::Model { choice, .. } => {
                line_of_row[row_idx] = Some(lines.len());
                let is_active = panel.is_model_active(row_idx);
                let is_cursor = panel.cursor() == row_idx;

                let check = if is_active { "\u{25cf}" } else { "\u{25cb}" };
                let cursor_char = if is_cursor { "\u{276f}" } else { " " };

                let label_style = if is_active {
                    Style::default()
                        .fg(theme::NEXUM_PRIMARY)
                        .add_modifier(Modifier::BOLD)
                } else if is_cursor {
                    Style::default()
                        .fg(theme::THINKING)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::TEXT)
                };

                let check_style = if is_active {
                    Style::default().fg(theme::NEXUM_PRIMARY)
                } else {
                    Style::default().fg(theme::MUTED)
                };

                let model_name = models_ref
                    .and_then(|m| m.get_model(choice.key.as_str()))
                    .unwrap_or("");

                let mut spans = vec![
                    Span::styled(
                        format!("   {} ", cursor_char),
                        Style::default().fg(theme::THINKING),
                    ),
                    Span::styled(format!("{}  ", check), check_style),
                    Span::styled(choice.label.clone(), label_style),
                ];
                if !model_name.is_empty() && model_name != choice.label {
                    spans.push(Span::styled(
                        format!("  {}", model_name),
                        Style::default().fg(theme::MUTED),
                    ));
                }
                lines.push(Line::from(spans));
            }
        }
    }

    lines.push(Line::from(""));

    // MaxTokens row — Ollama: show "local" instead of fake presets
    {
        line_of_row[panel.max_tokens_row()] = Some(lines.len());
        let is_cursor = panel.cursor() == panel.max_tokens_row();
        let radio_color = if is_cursor {
            theme::THINKING
        } else {
            theme::ACCENT
        };
        let label_style = if is_cursor {
            Style::default()
                .fg(theme::THINKING)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::BOLD)
        };
        let cursor_char = if is_cursor { "\u{276f}" } else { " " };

        let max_tokens_text = if is_ollama {
            format!("{}: local", lc.tr("model-field-max-token"))
        } else {
            format!(
                "{}: {}",
                lc.tr("model-field-max-token"),
                panel.buf_max_tokens
            )
        };

        let spans = vec![
            Span::styled(
                format!(" {} \u{25cf} ", cursor_char),
                Style::default().fg(radio_color),
            ),
            Span::styled(max_tokens_text, label_style),
        ];

        lines.push(Line::from(spans));
    }

    // Effort row — Ollama: show "N/A" (local models don't have configurable effort)
    {
        line_of_row[panel.effort_row()] = Some(lines.len());
        let is_cursor = panel.cursor() == panel.effort_row();
        let radio_color = if is_cursor {
            theme::THINKING
        } else {
            theme::ACCENT
        };
        let effort_style = if is_cursor {
            Style::default()
                .fg(theme::THINKING)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::BOLD)
        };
        let cursor_char = if is_cursor { "\u{276f}" } else { " " };

        let effort_text = if is_ollama {
            format!("{}: local", lc.tr("model-field-effort"))
        } else {
            let effort_key = match panel.buf_thinking_effort.as_str() {
                "low" => "model-effort-low",
                "high" => "model-effort-high",
                "xhigh" => "model-effort-xhigh",
                "max" => "model-effort-max",
                _ => "model-effort-medium",
            };
            format!("{}: {}", lc.tr("model-field-effort"), lc.tr(effort_key))
        };

        let spans = vec![
            Span::styled(
                format!(" {} \u{25cf} ", cursor_char),
                Style::default().fg(radio_color),
            ),
            Span::styled(effort_text, effort_style),
        ];

        lines.push(Line::from(spans));
    }

    // 1M Context row — Ollama: show "local" instead of ON/OFF
    {
        line_of_row[panel.context_1m_row()] = Some(lines.len());
        let is_cursor = panel.cursor() == panel.context_1m_row();
        let radio_color = if is_cursor {
            theme::THINKING
        } else {
            theme::ACCENT
        };
        let label_style = if is_cursor {
            Style::default()
                .fg(theme::THINKING)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::BOLD)
        };
        let cursor_char = if is_cursor { "\u{276f}" } else { " " };

        let (state_label, state_color) = if is_ollama {
            ("local".to_string(), theme::MUTED)
        } else if panel.buf_context_1m {
            ("ON".to_string(), theme::SAGE)
        } else {
            ("OFF".to_string(), theme::MUTED)
        };

        let spans = vec![
            Span::styled(
                format!(" {} \u{25cf} ", cursor_char),
                Style::default().fg(radio_color),
            ),
            Span::styled(
                format!("{}: ", lc.tr("model-field-1m-context")),
                label_style,
            ),
            Span::styled(
                state_label,
                Style::default()
                    .fg(state_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));

    // ── Viewport con scroll (Bug 1): el offset sigue al cursor ──────────────
    let total = lines.len();
    let content_total = content_line_count(&lines);
    let viewport = inner.height as usize;
    let cursor_line = line_of_row
        .get(panel.cursor())
        .copied()
        .flatten()
        .unwrap_or(0);
    // Margen de contexto (scrolloff): al subir, arrastrar también las 2
    // líneas de arriba (descripción/header de sección). Con el cursor en el
    // primer ítem el offset vuelve a 0 y el ▲ desaparece.
    let offset = scroll::ensure_visible(
        panel.scroll_offset,
        cursor_line.saturating_sub(2),
        cursor_line,
        viewport,
        total,
    );
    panel.scroll_offset = offset;
    let visible: Vec<Line> = lines.into_iter().skip(offset).take(viewport).collect();
    f.render_widget(Paragraph::new(Text::from(visible)), inner);
    paint_popup_background(f, area);
    // Para los indicadores, las líneas en blanco del final no cuentan como
    // "contenido oculto": el ▼ debe desaparecer al llegar al último ítem.
    render_overflow_indicators(f, inner, offset, viewport, content_total);
}

/// Cantidad de líneas hasta el último contenido real (excluye blancos finales).
pub(super) fn content_line_count(lines: &[Line]) -> usize {
    lines
        .iter()
        .rposition(|l| l.spans.iter().any(|s| !s.content.trim().is_empty()))
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Flechitas ▲/▼ en el borde derecho cuando hay contenido fuera de vista.
pub(super) fn render_overflow_indicators(
    f: &mut Frame,
    inner: Rect,
    offset: usize,
    viewport: usize,
    total: usize,
) {
    let (up, down) = scroll::overflow_indicators(offset, viewport, total);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let x = inner.right().saturating_sub(1);
    if up {
        f.render_widget(
            Paragraph::new(Span::styled("▲", Style::default().fg(theme::MUTED))),
            Rect::new(x, inner.top(), 1, 1),
        );
    }
    if down {
        f.render_widget(
            Paragraph::new(Span::styled("▼", Style::default().fg(theme::MUTED))),
            Rect::new(x, inner.bottom().saturating_sub(1), 1, 1),
        );
    }
}

fn paint_popup_background(f: &mut Frame, area: Rect) {
    let buf = f.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.bg = theme::POPUP_BG;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{model_panel::ModelPanel, App};
    include!("model_test.rs");
}
