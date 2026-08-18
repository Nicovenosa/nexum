// table/wrap.rs —— CJK 视觉宽度换行算法 + make_border 边框工具
//
// 这两个是纯算法无状态（make_border 是自由函数，wrap_cell_text 仅读 &self 不写），
// 是最易独立测试的部分。建议未来新增 table/wrap_test.rs 覆盖 CJK 断行边界用例
// （当前未被直接单测，是隐性风险点）。
//
// [TRAP] 字符串截断：wrap_cell_text 已正确使用 char_indices + UnicodeWidthChar，
//        禁止改用 &s[..N] 字节切片，否则 CJK 会 panic。
//        CLAUDE.md 明确要求 chars 级操作。
// [不变量] 至少推进一个字符：单字符超宽（CJK 在窄列）必须强制放入，否则死循环。
//
// wrap_cell_text 是 TableBuilder 的 impl（Rust 允许跨文件拆 impl 块）。

use ratatui::{style::Style, text::Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::builder::TableBuilder;
use crate::markdown::MarkdownTheme;

impl TableBuilder {
    /// 包装单个单元格的文本，按视觉宽度折行（正确支持 CJK 双宽字符）
    pub(super) fn wrap_cell_text(
        &self,
        cell: &[Span],
        max_width: usize,
    ) -> Vec<Vec<Span<'static>>> {
        if max_width == 0 || cell.is_empty() {
            return vec![vec![]];
        }

        // 合并单元格中的所有文本
        let full_text: String = cell.iter().map(|s| s.content.as_ref()).collect();
        let base_style = cell.first().map(|s| s.style).unwrap_or_default();

        if full_text.width() <= max_width {
            // 不需要换行，将所有 Span 转换为 'static
            let static_spans: Vec<Span<'static>> = cell
                .iter()
                .map(|s| Span::styled(s.content.as_ref().to_string(), s.style))
                .collect();
            return vec![static_spans];
        }

        // 需要换行 — 用视觉宽度逐字符推进
        let mut lines = Vec::new();
        let text = full_text.as_str();
        let mut byte_pos = 0;

        while byte_pos < text.len() {
            // 从 byte_pos 开始，累积视觉宽度直到超过 max_width
            let mut cur_width = 0usize;
            let mut content_end = byte_pos; // 按宽度截断的字节位置

            for (i, c) in text[byte_pos..].char_indices() {
                let cw = c.width().unwrap_or(0);
                // 至少推进一个字符，防止单字符超宽死循环
                if content_end > byte_pos && cur_width + cw > max_width {
                    break;
                }
                // 字符本身超宽（如 CJK 在很窄的列里），强制放入
                cur_width += cw;
                content_end = byte_pos + i + c.len_utf8();
            }

            // 从 content_end 往回找最后一个空格，优先在单词边界断行
            let mut break_at = content_end;
            for (i, c) in text[byte_pos..content_end].char_indices().rev() {
                if c.is_whitespace() {
                    break_at = byte_pos + i;
                    break;
                }
            }

            let line = &text[byte_pos..break_at];
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                lines.push(vec![Span::styled(trimmed.to_string(), base_style)]);
            }

            byte_pos = break_at;
            // 跳过后续空白
            while byte_pos < text.len() {
                let c = text[byte_pos..].chars().next().unwrap();
                if c.is_whitespace() {
                    byte_pos += c.len_utf8();
                } else {
                    break;
                }
            }
        }

        if lines.is_empty() {
            lines.push(vec![]);
        }

        lines
    }
}

/// 表格边框工具函数：构造 ┌┬┐├┼┤└┴┘ 等行
pub(super) fn make_border(
    col_widths: &[usize],
    left: char,
    mid: char,
    right: char,
    fill: char,
    theme: &dyn MarkdownTheme,
) -> Span<'static> {
    let mut s = String::new();
    s.push(left);
    for (i, &w) in col_widths.iter().enumerate() {
        for _ in 0..w + 2 {
            s.push(fill);
        }
        if i < col_widths.len() - 1 {
            s.push(mid);
        }
    }
    s.push(right);
    Span::styled(s, Style::default().fg(theme.muted()))
}
