use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};

use crate::{app::App, ui::theme};

/// 浅色背景色（不影响整体终端背景，只在文字区域可见）
const HEADER_BG: ratatui::style::Color = theme::USER_BG;

/// ¿Está habilitada la "active prompt bar" (sticky header con el último
/// prompt del usuario fijado arriba del transcript)?
///
/// Default: **OFF** en Nexum (sprint prompt-sticky 2026-07-07). Opt-in
/// explícito con `NEXUM_ACTIVE_PROMPT_BAR=on|1|true|yes`.
///
/// Bug que resuelve: durante una generación larga (el transcript excede el
/// viewport → `max_scroll > 0`) esta barra aparecía mostrando el último
/// mensaje del usuario, DUPLICÁNDOLO: una vez como UserBubble normal en el
/// transcript y otra fija arriba con fondo resaltado (`USER_BG` + `❯`),
/// pareciendo una barra activa que no se iba hasta terminar la ejecución.
/// El prompt enviado debe quedar como mensaje normal del transcript, punto.
pub fn active_prompt_bar_enabled() -> bool {
    if crate::ui::demo_mode::public_demo_enabled() {
        return false;
    }
    matches!(
        std::env::var("NEXUM_ACTIVE_PROMPT_BAR")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "on" | "1" | "true" | "yes"
    )
}

/// 渲染 sticky human message header
pub fn render_sticky_header(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 || !active_prompt_bar_enabled() {
        return;
    }

    let msg = match &app.session_mgr.current().metadata.last_human_message {
        Some(m) => m,
        None => return,
    };

    // 可用宽度（留 padding）
    let width = area.width.saturating_sub(4).max(1) as usize;
    // 可显示内容行数
    let max_lines = area.height.max(1) as usize;

    // 将消息文本按宽度分多行
    let wrapped_lines = wrap_message(msg, width, max_lines);

    // 每行文字都有浅背景，无分隔线
    let bg_style = Style::default().bg(HEADER_BG);
    let text_style = Style::default().fg(theme::TEXT).bg(HEADER_BG);
    let label_style = Style::default()
        .fg(theme::ACCENT)
        .add_modifier(Modifier::BOLD)
        .bg(HEADER_BG);

    let lines: Vec<Line> = wrapped_lines
        .into_iter()
        .map(|text| {
            Line::from(vec![
                Span::styled("❯ ", label_style),
                Span::styled(text, text_style),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(Text::from(lines)).style(bg_style);
    f.render_widget(paragraph, area);
}

/// 根据终端宽度估算消息占用的视觉行数（用于 Layout 高度计算）
pub(super) fn estimate_header_lines(msg: &str, width: u16) -> usize {
    if width == 0 {
        return 1;
    }
    let width = width as usize;
    let char_count = msg.chars().count();
    let lines = char_count.div_ceil(width);
    lines.clamp(1, 3)
}

/// 将消息文本按宽度分多行（用于渲染）
fn wrap_message(msg: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 {
        return vec![];
    }

    let chars: Vec<char> = msg.chars().collect();
    let total_chars = chars.len();
    let mut result = Vec::new();
    let mut pos = 0;

    while pos < total_chars && result.len() < max_lines {
        let remaining = total_chars - pos;
        let chunk_size = width.min(remaining);

        if pos + chunk_size >= total_chars {
            result.push(chars[pos..].iter().collect());
            break;
        }

        let chunk_chars = &chars[pos..pos + chunk_size];
        let break_idx = chunk_chars
            .iter()
            .rposition(|&c| c.is_ascii_whitespace() || c == '　')
            .map(|i| i + 1)
            .unwrap_or(chunk_size);

        let line_text: String = chars[pos..pos + break_idx].iter().collect();
        result.push(line_text);
        pos += break_idx;

        while pos < total_chars && (chars[pos].is_ascii_whitespace() || chars[pos] == '　') {
            pos += 1;
        }
    }

    // 截断时在最后一行末尾加 …
    if pos < total_chars && result.len() == max_lines {
        if let Some(last) = result.last_mut() {
            let trimmed = last.trim_end();
            if !trimmed.is_empty() && !trimmed.ends_with('…') {
                let char_count = trimmed.chars().count();
                if char_count > 2 {
                    let suffix_start = char_count.saturating_sub(2);
                    let suffix: String = trimmed.chars().skip(suffix_start).collect();
                    let prefix: String = trimmed.chars().take(suffix_start).collect();
                    *last = format!("{}{}…", prefix, suffix.chars().next().unwrap_or(' '));
                }
            }
        }
    }

    result
}
// El flag `active_prompt_bar_enabled()` se testea end-to-end en headless_test.rs
// (`test_active_prompt_bar_off_por_default_no_sticky` /
// `test_active_prompt_bar_on_muestra_sticky`), bajo el lock compartido de env
// para evitar races con los otros tests que tocan NEXUM_ACTIVE_PROMPT_BAR. No
// se duplica acá un test unitario del parseo (mismo patrón que
// composer::predictions_enabled, ya cubierto) porque un lock local separado
// carreaba con los async tests sobre la misma env var.
