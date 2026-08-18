use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Widget, WidgetRef},
};

/// OSC 8 超链接的 Span 工厂。
///
/// 直接注入 OSC 8 escape sequence 到 Span 文本中，不依赖 ratatui `unstable-hyperlink` feature。
/// 支持终端：iTerm2、Terminal.app、Windows Terminal、WezTerm、Alacritty、Kitty、foot 等。
pub struct LinkSpan {
    url: String,
    text: String,
    style: Style,
    max_width: Option<u16>,
}

impl LinkSpan {
    /// 创建 LinkSpan。若 text 为空，使用 url 作为 fallback 文本。
    pub fn new(url: impl Into<String>, text: impl Into<String>) -> Self {
        let url: String = url.into();
        let text: String = text.into();
        let text = if text.is_empty() { url.clone() } else { text };
        Self {
            url,
            text,
            style: Style::default().add_modifier(Modifier::UNDERLINED),
            max_width: None,
        }
    }

    /// 设置自定义样式（覆盖默认的 UNDERLINED）。
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// 设置最大显示宽度（字符数，CJK 安全）。超出截断并追加 "…"。
    pub fn max_width(mut self, w: u16) -> Self {
        self.max_width = Some(w);
        self
    }

    /// 产出带 OSC 8 hyperlink 的 Span。
    /// URL 为空时只返回纯文本 Span（无 OSC 8 序列）。
    pub fn to_span(&self) -> Span<'static> {
        let display_text = match self.max_width {
            None => self.text.clone(),
            Some(w) if self.text.chars().count() <= w as usize => self.text.clone(),
            Some(w) => {
                let mut truncated: String = self.text.chars().take(w as usize).collect();
                truncated.push('…');
                truncated
            }
        };

        let text = if self.url.is_empty() {
            display_text
        } else {
            let safe_url = sanitize_url(&self.url);
            format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", safe_url, display_text)
        };

        Span::styled(text, self.style)
    }
}

/// 过滤 URL 中的控制字符，防止 OSC 8 注入攻击。
/// 保留可打印 ASCII、Unicode 字符、常见 URL 合法字符。
/// 过滤掉：0x00-0x1F（控制字符，不含 \t）、0x7F（DEL）、\x1b（ESC）。
fn sanitize_url(url: &str) -> String {
    url.chars()
        .filter(|&c| c >= ' ' || c == '\t')
        .filter(|&c| c != '\x1b' && c != '\x7f')
        .collect()
}

/// 独立场景下的薄 Widget 包装。
pub struct LinkWidget<'a> {
    pub link: &'a LinkSpan,
}

impl WidgetRef for LinkWidget<'_> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let line = Line::from(self.link.to_span());
        ratatui::widgets::Paragraph::new(line).render(area, buf);
    }
}

impl Widget for LinkWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_ref(area, buf);
    }
}

/// 将文本包裹上 OSC 8 超链接 escape sequences。
/// 用于 Markdown coordinator 内联场景，直接产出带 OSC 8 的文本。
pub fn wrap_osc8(text: &str, url: &str) -> String {
    if url.is_empty() {
        text.to_string()
    } else {
        let safe_url = sanitize_url(url);
        format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", safe_url, text)
    }
}

#[cfg(test)]
#[path = "link_test.rs"]
mod tests;
