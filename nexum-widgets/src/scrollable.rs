use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Span, Text},
    widgets::{Paragraph, Wrap},
    Frame,
};

/// 自定义垂直滚动条渲染：用 bg 颜色空格替代 ratatui Scrollbar widget 的
/// █ 字符。█ 在部分终端有行间缝隙；空格 + bg 颜色形成连续色块。
///
/// 返回 (up_btn_area, down_btn_area) 供鼠标交互。
pub fn render_vertical_scrollbar(
    f: &mut Frame,
    bar_area: Rect,
    offset: u16,
    max_scroll: u16,
    style: Style,
    max_thumb_len: Option<u16>,
    show_arrows: bool,
) -> (Option<Rect>, Option<Rect>) {
    let has_up = show_arrows && offset > 0;
    let has_down = show_arrows && offset < max_scroll && bar_area.height > 1;
    let arrow_slots: u16 = (has_up as u16) + (has_down as u16);

    let track_start = if has_up { bar_area.y + 1 } else { bar_area.y };
    let track_height = bar_area.height.saturating_sub(arrow_slots);
    let visible_height = track_height; // track 即 thumb 的活动范围

    // Thumb 尺寸（比例 = visible / content）
    let thumb_raw = if track_height > 0 && max_scroll > 0 {
        let content_height = max_scroll + visible_height;
        let raw = (track_height as u64 * visible_height as u64) / content_height as u64;
        (raw as u16).max(1)
    } else if track_height > 0 {
        track_height // 内容未溢出，thumb 占满
    } else {
        0
    };
    let thumb_size = if let Some(max_thumb) = max_thumb_len {
        thumb_raw.min(max_thumb).max(1)
    } else {
        thumb_raw
    };
    let thumb_start_offset = if max_scroll > 0 && track_height > thumb_size {
        let pos =
            (track_height.saturating_sub(thumb_size) as u64 * offset as u64) / max_scroll as u64;
        pos as u16
    } else {
        0
    };

    let thumb_bg = style
        .fg
        .unwrap_or(ratatui::style::Color::Rgb(148, 163, 184));
    let buf: &mut Buffer = f.buffer_mut();

    // 渲染 track：空格无 bg（透明）
    for row in 0..track_height {
        let y = track_start + row;
        if let Some(cell) = buf.cell_mut((bar_area.x, y)) {
            cell.set_symbol(" ");
        }
    }

    // 渲染 thumb：空格 + bg 颜色
    for row in 0..thumb_size {
        let y = track_start + thumb_start_offset + row;
        if y < bar_area.bottom() {
            if let Some(cell) = buf.cell_mut((bar_area.x, y)) {
                cell.set_symbol(" ");
                cell.bg = thumb_bg;
            }
        }
    }

    // ▲ 按钮
    let up_btn_area = if has_up {
        let btn = Rect {
            x: bar_area.x,
            y: bar_area.y,
            width: 1,
            height: 1,
        };
        let arrow = Paragraph::new(Text::from(Span::styled(
            "▲",
            style.add_modifier(Modifier::BOLD),
        )));
        f.render_widget(arrow, btn);
        Some(btn)
    } else {
        None
    };

    // ▼ 按钮
    let down_btn_area = if has_down {
        let btn = Rect {
            x: bar_area.x,
            y: bar_area.bottom().saturating_sub(1),
            width: 1,
            height: 1,
        };
        let arrow = Paragraph::new(Text::from(Span::styled(
            "▼",
            style.add_modifier(Modifier::BOLD),
        )));
        f.render_widget(arrow, btn);
        Some(btn)
    } else {
        None
    };

    (up_btn_area, down_btn_area)
}
#[derive(Debug, Clone, Copy)]
pub struct ScrollbarMetrics {
    /// 滚动条列区域（宽 1，面板全高）
    pub bar_area: Rect,
    /// 最大滚动偏移量
    pub max_offset: u16,
    /// ▲ 按钮区域（offset > 0 时显示）
    pub up_btn_area: Option<Rect>,
    /// ▼ 按钮区域（offset < max_offset 时显示）
    pub down_btn_area: Option<Rect>,
}

/// 滚动偏移状态
///
/// 管理垂直滚动 offset，提供 ensure_visible 方法自动调整 offset 使指定行可见。
/// 逻辑从 `nexum-tui/src/app/mod.rs:ensure_cursor_visible()` 迁移。
#[derive(Debug, Clone, Default)]
pub struct ScrollState {
    offset: u16,
}

impl ScrollState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_offset(offset: u16) -> Self {
        Self { offset }
    }

    pub fn offset(&self) -> u16 {
        self.offset
    }

    /// 向上滚动 delta 行
    pub fn scroll_up(&mut self, delta: u16) {
        self.offset = self.offset.saturating_sub(delta);
    }

    /// 向下滚动 delta 行（不超过 max_scroll）
    pub fn scroll_down(&mut self, delta: u16, content_height: u16, visible_height: u16) {
        let max_scroll = content_height.saturating_sub(visible_height);
        self.offset = (self.offset + delta).min(max_scroll);
    }

    /// 确保 row 行在可见视口内，自动调整 offset
    ///
    /// 从 `ensure_cursor_visible(cursor_row, scroll_offset, visible_height)` 迁移。
    pub fn ensure_visible(&mut self, row: u16, visible_height: u16) {
        if visible_height == 0 {
            self.offset = 0;
            return;
        }
        if row < self.offset {
            self.offset = row;
        } else if row >= self.offset + visible_height {
            self.offset = row.saturating_sub(visible_height.saturating_sub(1));
        }
    }

    pub fn reset(&mut self) {
        self.offset = 0;
    }
}

/// 可滚动区域——内容 + 可选滚动条
pub struct ScrollableArea<'a> {
    content: Text<'a>,
    show_scrollbar: bool,
    scrollbar_style: Style,
    max_thumb_length: Option<u16>,
}

impl<'a> ScrollableArea<'a> {
    pub fn new(content: Text<'a>) -> Self {
        Self {
            content,
            show_scrollbar: true,
            scrollbar_style: Style::default(),
            max_thumb_length: None,
        }
    }

    pub fn show_scrollbar(mut self, show: bool) -> Self {
        self.show_scrollbar = show;
        self
    }

    pub fn scrollbar_style(mut self, style: Style) -> Self {
        self.scrollbar_style = style;
        self
    }

    /// 限制滚动条滑块（thumb）的最大高度（行数）
    pub fn max_thumb_length(mut self, max: u16) -> Self {
        self.max_thumb_length = Some(max);
        self
    }

    /// 渲染可滚动区域：Paragraph + 可选 Scrollbar
    ///
    /// 自动根据内容高度和可见高度决定是否显示滚动条。
    /// 内容区域宽度减 1 留给滚动条（当 scrollbar 显示时）。
    pub fn render(
        self,
        f: &mut Frame,
        area: Rect,
        state: &mut ScrollState,
    ) -> Option<ScrollbarMetrics> {
        let content_height = self.content.height() as u16;
        let visible_height = area.height;
        let max_scroll = content_height.saturating_sub(visible_height);
        // clamp offset
        state.offset = state.offset.min(max_scroll);

        let needs_scrollbar = self.show_scrollbar && content_height > visible_height;
        let text_width = if needs_scrollbar {
            area.width.saturating_sub(1)
        } else {
            area.width
        };
        let text_area = Rect {
            width: text_width,
            ..area
        };

        let paragraph = Paragraph::new(self.content)
            .scroll((state.offset, 0))
            .wrap(Wrap { trim: false });
        f.render_widget(paragraph, text_area);

        if needs_scrollbar {
            let bar_area = Rect {
                x: area.right().saturating_sub(1),
                y: area.y,
                width: 1,
                height: area.height,
            };

            let (up_btn_area, down_btn_area) = render_vertical_scrollbar(
                f,
                bar_area,
                state.offset,
                max_scroll,
                self.scrollbar_style,
                self.max_thumb_length,
                true, // show_arrows
            );

            Some(ScrollbarMetrics {
                bar_area,
                max_offset: max_scroll,
                up_btn_area,
                down_btn_area,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
#[path = "scrollable_test.rs"]
mod tests;
