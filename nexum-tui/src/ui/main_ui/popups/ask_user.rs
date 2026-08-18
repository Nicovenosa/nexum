use nexum_widgets::{BorderedPanel, ScrollState, ScrollableArea, TabBar, TabState, TabStyle};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::{app::App, ui::theme};

/// AskUser 批量弹窗（底部展开区）：header tab 行 + 编号选项列表
///
/// 风格对齐 Claude Code 原生 AskUser UI：
/// - Tab 栏带 ☐/✔ 状态标记
/// - 选项编号格式（单选: `❯ 1. label`，多选: `❯ ● 1. label`）
/// - 自定义输入合并为最后一个编号选项（使用 FieldTextarea overlay）
pub(crate) fn render_ask_user_popup(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(crate::app::InteractionPrompt::Questions(prompt)) =
        &app.session_mgr.current_mut().agent.interaction_prompt
    else {
        return;
    };

    let cur = &prompt.questions[prompt.active_tab];

    let inner = BorderedPanel::new(Span::styled("", Style::default()))
        .border_style(Style::default().fg(theme::THINKING))
        .render(f, area);

    // ── header 行：每个问题一个 tab，已确认 ✔ 未确认 ☐ ─────────────────────
    let header_area = Rect { height: 1, ..inner };
    let labels: Vec<String> = prompt
        .questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let done = prompt.confirmed.get(i).copied().unwrap_or(false);
            let header = if q.data.header.is_empty() {
                format!("Q{}", i + 1)
            } else {
                q.data.header.chars().take(10).collect()
            };
            if done {
                format!("✔ {}", header)
            } else {
                format!("☐ {}", header)
            }
        })
        .collect();
    let mut tab_state = TabState::new(labels);
    tab_state.set_active(prompt.active_tab);
    let tab_bar = TabBar::new().style(TabStyle {
        active: Style::default()
            .fg(Color::White)
            .bg(theme::THINKING)
            .add_modifier(Modifier::BOLD),
        completed: Style::default().fg(theme::SAGE),
        incomplete: Style::default().fg(theme::MUTED),
        separator: " ",
    });
    f.render_stateful_widget(tab_bar, header_area, &mut tab_state);

    // ── 分隔线 ────────────────────────────────────────────────────────────────
    let sep_area = Rect {
        y: inner.y + 1,
        height: 1,
        ..inner
    };
    let sep = "─".repeat(inner.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(sep, Style::default().fg(theme::MUTED))),
        sep_area,
    );

    // ── 内容区 ────────────────────────────────────────────────────────────────
    let content_area = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let mut lines: Vec<Line> = Vec::new();
    let mut option_row_map: Vec<u16> = Vec::new();

    // 问题文本
    for l in cur.data.question.lines() {
        lines.push(Line::from(Span::styled(
            l.to_string(),
            Style::default().fg(theme::TEXT),
        )));
    }
    lines.push(Line::from(""));

    // 选项列表（编号格式）
    let multi = cur.data.multi_select;
    let option_count = cur.data.options.len();
    for (i, opt) in cur.data.options.iter().enumerate() {
        option_row_map.push(lines.len() as u16);
        let is_cursor = !cur.in_custom_input && cur.option_cursor == i as isize;
        let is_selected = cur.selected.get(i).copied().unwrap_or(false);
        let num = i + 1;
        let cursor_mark = if is_cursor { "❯" } else { " " };

        let row_style = if is_cursor {
            Style::default()
                .fg(theme::THINKING)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(theme::SAGE)
        } else {
            Style::default().fg(theme::TEXT)
        };

        if multi {
            let check = if is_selected { "●" } else { "○" };
            lines.push(Line::from(vec![
                Span::styled(format!("{} {} {}. ", cursor_mark, check, num), row_style),
                Span::styled(opt.label.clone(), row_style),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(format!("{} {}. ", cursor_mark, num), row_style),
                Span::styled(opt.label.clone(), row_style),
            ]));
        }

        // 选项 description（若有）
        if let Some(ref desc) = opt.description {
            if !desc.is_empty() {
                let indent = if multi { "       " } else { "     " };
                lines.push(Line::from(Span::styled(
                    format!("{}{}", indent, desc),
                    Style::default().fg(theme::MUTED),
                )));
            }
        }

        // 选项之间空一行（最后一个不加）
        if i < option_count - 1 {
            lines.push(Line::from(""));
        }
    }

    // 自定义输入前加空行分隔
    lines.push(Line::from(""));

    // 记录 textarea label 在 lines 中的逻辑行索引
    let textarea_label_line = lines.len() as u16;

    // 自定义输入 label 行（前缀 "❯ N. "）
    let custom_num = option_count + 1;
    let is_cursor = cur.in_custom_input;
    let cursor_mark = if is_cursor { "❯" } else { " " };
    let style = if is_cursor {
        Style::default()
            .fg(theme::THINKING)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::MUTED)
    };
    let prefix_str = format!("{} {}. ", cursor_mark, custom_num);
    let prefix_width = UnicodeWidthStr::width(prefix_str.as_str()) as u16;

    if cur.custom_input.is_empty() && !is_cursor {
        // 空且未聚焦：显示 placeholder（无需 textarea overlay）
        let placeholder = app.services.lc.tr("ask-user-placeholder");
        lines.push(Line::from(vec![
            Span::styled(prefix_str, style),
            Span::styled(placeholder, style),
        ]));
    } else {
        // 有内容或已聚焦：label 行 + textarea overlay 占位
        lines.push(Line::from(Span::styled(prefix_str.clone(), style)));
        // 为 textarea 额外行数预留空行（第 1 行在 label 行内，额外需要 render_height - 1 行）
        let extra = cur.custom_input.render_height().saturating_sub(1);
        for _ in 0..extra {
            lines.push(Line::from(""));
        }
    }

    // 保存 overlay 参数（cur 借用在 ScrollableArea 后失效）
    let needs_overlay = !cur.custom_input.is_empty() || is_cursor;
    let ta_render_height = cur.custom_input.render_height();
    let scroll_offset = prompt.scroll_offset;

    // ── 计算 textarea label 行的视觉行偏移（考虑 ScrollableArea 的 word wrapping）──
    // ScrollableArea 内部用 Paragraph + Wrap{trim:false} 渲染，逻辑行索引 ≠ 视觉行位置。
    // 当前置内容（问题文本、选项等）因面板宽度发生换行时，逻辑行索引会产生偏移。
    // 使用 Paragraph::line_count()（与 ratatui WordWrapper 算法完全一致）精确计算。
    let text_width = if content_area.height < lines.len() as u16 {
        content_area.width.saturating_sub(1) // 有滚动条时少 1 列
    } else {
        content_area.width
    };
    let visual_label_offset: u16 = if textarea_label_line == 0 || text_width == 0 {
        0
    } else {
        let prefix_text: Vec<Line> = lines[..textarea_label_line as usize].to_vec();
        Paragraph::new(Text::from(prefix_text))
            .wrap(Wrap { trim: false })
            .line_count(text_width) as u16
    };

    let mut scroll_state = ScrollState::with_offset(scroll_offset);
    let metrics = ScrollableArea::new(Text::from(lines))
        .scrollbar_style(Style::default().fg(theme::MUTED))
        .render(f, content_area, &mut scroll_state);
    app.session_mgr.current_mut().ui.panel_scrollbar_metrics = metrics;
    // 存储面板区域供鼠标事件路由
    app.session_mgr.current_mut().ui.panel_area = Some(area);
    if let Some(crate::app::InteractionPrompt::Questions(p)) = app
        .session_mgr
        .current_mut()
        .agent
        .interaction_prompt
        .as_mut()
    {
        p.scrollbar_metrics = metrics;
        p.option_row_map = option_row_map;
    }

    // ── textarea overlay：在 label 行右侧渲染 FieldTextarea widget ──────────
    if needs_overlay {
        let visible_label_y = content_area.y + visual_label_offset.saturating_sub(scroll_offset);
        if visible_label_y >= content_area.y
            && visible_label_y < content_area.y + content_area.height
        {
            let ta_height =
                ta_render_height.min(content_area.bottom().saturating_sub(visible_label_y));
            let textarea_area = Rect {
                x: content_area.x + prefix_width,
                y: visible_label_y,
                width: content_area.width.saturating_sub(prefix_width),
                height: ta_height,
            };
            if textarea_area.width > 0 && textarea_area.height > 0 {
                if let Some(crate::app::InteractionPrompt::Questions(p)) = app
                    .session_mgr
                    .current_mut()
                    .agent
                    .interaction_prompt
                    .as_mut()
                {
                    p.current().custom_input.render(f, textarea_area);
                }
            }
        }
    }
}
