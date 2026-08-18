use nexum_widgets::BorderedPanel;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};

use crate::{
    app::{betas_panel::BetasPanel, App},
    ui::theme,
};

pub(crate) fn render_betas_panel(f: &mut Frame, panel: &BetasPanel, app: &mut App, area: Rect) {
    let title = " Beta \u{529f}\u{80fd}\u{5f00}\u{5173} ";

    let inner = BorderedPanel::new(Span::styled(
        title,
        Style::default()
            .fg(theme::THINKING)
            .add_modifier(Modifier::BOLD),
    ))
    .border_style(Style::default().fg(theme::BORDER))
    .render(f, area);

    app.session_mgr.current_mut().ui.panel_area = Some(inner);

    let mut lines: Vec<Line> = Vec::new();

    // 顶部提示（灰色）
    lines.push(Line::from(Span::styled(
        "  \u{53d8}\u{66f4}\u{5c06}\u{5728}\u{65b0}\u{4f1a}\u{8bdd}\u{4e2d}\u{751f}\u{6548}",
        Style::default().fg(theme::MUTED),
    )));
    // 空行分隔
    lines.push(Line::from(""));

    let desc_style = Style::default().fg(theme::MUTED);

    for (i, entry) in panel.entries.iter().enumerate() {
        let is_cursor = i == panel.cursor;
        let cursor_char = if is_cursor { "\u{276f} " } else { "  " };
        let label_style = if is_cursor {
            Style::default()
                .fg(theme::THINKING)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT)
        };

        let value_style = if entry.enabled {
            Style::default()
                .fg(theme::SAGE)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::MUTED)
        };
        let value_text = if entry.enabled { "on" } else { "off" };

        lines.push(Line::from(vec![
            Span::styled(
                cursor_char.to_string(),
                Style::default().fg(theme::THINKING),
            ),
            Span::styled(format!("{:<14}", entry.label), label_style),
            Span::styled(value_text.to_string(), value_style),
        ]));
        lines.push(Line::from(Span::styled(
            format!("      {}", entry.description),
            desc_style,
        )));
    }

    lines.truncate(inner.height as usize);
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}
