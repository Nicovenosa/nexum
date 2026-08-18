use ratatui::{
    layout::Rect,
    text::Span,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::{app::App, ui::theme};

pub(crate) fn render_oauth_popup(f: &mut Frame, app: &mut App, area: Rect) {
    let lc = &app.services.lc;
    let prompt = match app.global_ui.oauth_prompt.as_ref() {
        Some(p) => p,
        None => return,
    };

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });

    let title = lc.tr_args(
        "oauth-title",
        &[("server".into(), prompt.server_name.clone().into())],
    );
    let title_span = Span::styled(
        title,
        ratatui::style::Style::default()
            .fg(theme::THINKING)
            .add_modifier(ratatui::style::Modifier::BOLD),
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(ratatui::style::Style::default().fg(theme::BORDER))
        .title(title_span);

    f.render_widget(block, area);

    let mut lines: Vec<ratatui::text::Line> = Vec::new();

    // 提示行
    lines.push(ratatui::text::Line::from(vec![Span::styled(
        lc.tr("oauth-prompt"),
        ratatui::style::Style::default().fg(theme::TEXT),
    )]));

    // URL 行 — 单行，背景高亮
    let url_style = ratatui::style::Style::default()
        .fg(theme::SAGE)
        .bg(ratatui::style::Color::DarkGray);
    lines.push(ratatui::text::Line::from(vec![Span::styled(
        prompt.authorization_url.clone(),
        url_style,
    )]));

    // 空行
    lines.push(ratatui::text::Line::from(""));

    // 输入行
    let value = prompt.field.value();
    lines.push(ratatui::text::Line::from(vec![
        Span::styled(
            lc.tr("oauth-callback-label"),
            ratatui::style::Style::default().fg(theme::MUTED),
        ),
        Span::raw(format!("{}█", value)),
    ]));

    // 错误行
    if let Some(ref err) = prompt.error_message {
        lines.push(ratatui::text::Line::from(vec![Span::styled(
            err,
            ratatui::style::Style::default().fg(theme::ERROR),
        )]));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("oauth_test.rs");
}
