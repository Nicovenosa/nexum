// table/render.rs —— 表格渲染
//
// render_with_wrap 消费 wrap_cells（layout.rs）的结果，绘制边框（顶/中/底）
// + 对齐填充（Center/Right/None/Left）+ 多行单元格高度对齐。
//
// [TRAP] unicode-width 一致性：本文件渲染时用 .content.width() 计算 line_width/content_width，
//        与 layout.rs 的列宽算法必须统一，禁止改用 .len()。
//
// render_with_wrap 是 TableBuilder 的 impl（Rust 允许跨文件拆 impl 块）。

use pulldown_cmark::Alignment;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::builder::TableBuilder;
use super::wrap::make_border;
use crate::markdown::MarkdownTheme;

impl TableBuilder {
    /// 渲染表格，支持自动换行
    pub(in crate::markdown::render_state) fn render_with_wrap(
        self,
        max_width: usize,
        theme: &dyn MarkdownTheme,
    ) -> Vec<Line<'static>> {
        let wrapped_rows = self.wrap_cells(max_width, theme);
        if wrapped_rows.is_empty() {
            return vec![];
        }

        // 计算每列的最大宽度（考虑换行后的每行）
        let num_cols = wrapped_rows[0].len();
        let mut col_widths = vec![0usize; num_cols];

        for row in &wrapped_rows {
            for (col_idx, cell_lines) in row.iter().enumerate() {
                if col_idx < num_cols {
                    for line in cell_lines {
                        let line_width: usize = line.iter().map(|s| s.content.width()).sum();
                        col_widths[col_idx] = col_widths[col_idx].max(line_width);
                    }
                }
            }
        }

        let mut lines = Vec::new();

        // 顶部边框
        lines.push(Line::from(make_border(
            &col_widths,
            '┌',
            '┬',
            '┐',
            '─',
            theme,
        )));

        // 渲染每一行
        for (row_idx, row) in wrapped_rows.iter().enumerate() {
            // 计算这一行需要的行数（基于最高的单元格）
            let max_lines = row
                .iter()
                .map(|cell_lines| cell_lines.len())
                .max()
                .unwrap_or(1);

            for line_idx in 0..max_lines {
                let mut spans = Vec::new();
                spans.push(Span::styled(
                    "│".to_string(),
                    Style::default().fg(theme.muted()),
                ));

                for (col_idx, cell_lines) in row.iter().enumerate() {
                    let col_w = col_widths.get(col_idx).copied().unwrap_or(0);
                    spans.push(Span::raw(" "));

                    if line_idx < cell_lines.len() {
                        // 获取这一行的内容
                        let line_spans = &cell_lines[line_idx];
                        let content_width: usize =
                            line_spans.iter().map(|s| s.content.width()).sum();
                        let padding = col_w.saturating_sub(content_width);

                        let align = self
                            .alignments
                            .get(col_idx)
                            .copied()
                            .unwrap_or(Alignment::None);
                        match align {
                            Alignment::Center => {
                                let left_pad = padding / 2;
                                let right_pad = padding - left_pad;
                                if left_pad > 0 {
                                    spans.push(Span::raw(" ".repeat(left_pad)));
                                }
                                spans.extend(line_spans.iter().cloned());
                                if right_pad > 0 {
                                    spans.push(Span::raw(" ".repeat(right_pad)));
                                }
                            }
                            Alignment::Right => {
                                if padding > 0 {
                                    spans.push(Span::raw(" ".repeat(padding)));
                                }
                                spans.extend(line_spans.iter().cloned());
                            }
                            Alignment::None | Alignment::Left => {
                                spans.extend(line_spans.iter().cloned());
                                if padding > 0 {
                                    spans.push(Span::raw(" ".repeat(padding)));
                                }
                            }
                        }
                    } else {
                        // 这一行的这个单元格没有内容，填充空格
                        spans.push(Span::raw(" ".repeat(col_w)));
                    }

                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        "│".to_string(),
                        Style::default().fg(theme.muted()),
                    ));
                }

                lines.push(Line::from(spans));
            }

            // 在第一行后添加分隔线
            if row_idx == 0 {
                lines.push(Line::from(make_border(
                    &col_widths,
                    '├',
                    '┼',
                    '┤',
                    '─',
                    theme,
                )));
            }
        }

        // 底部边框
        lines.push(Line::from(make_border(
            &col_widths,
            '└',
            '┴',
            '┘',
            '─',
            theme,
        )));

        lines
    }
}
