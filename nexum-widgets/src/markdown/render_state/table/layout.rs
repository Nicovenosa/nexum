// table/layout.rs —— 列宽分配算法 + wrap_cells 编排
//
// 三个独立宽度算法 + wrap_cells 编排：
//   - calculate_min_col_widths：每列最小宽度（基于最短内容，上限 10）
//   - calculate_ideal_col_widths：每列理想宽度（基于内容长度）
//   - distribute_col_widths：保证最小宽度后按比例分配剩余空间（CJK 列至少 ~4-10 显示列宽）
//   - wrap_cells：编排 layout + wrap_cell_text（wrap.rs）产出每行每单元格的多行 span
//
// [TRAP] unicode-width 一致性：本文件用 .content.width()，render.rs 也必须用 .content.width()，
//        禁止任一处改用 .len()（字节长度）——CJK 双宽字符会让两处算法错位
//
// 这些方法都是 TableBuilder 的 impl（Rust 允许跨文件拆 impl 块）。

use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

// 从 markdown/render_state/table/ 回到 markdown 模块（table → render_state → markdown）
use crate::markdown::MarkdownTheme;

use super::builder::TableBuilder;

impl TableBuilder {
    /// 包装单元格文本以适应最大宽度
    pub(super) fn wrap_cells(
        &self,
        max_width: usize,
        _theme: &dyn MarkdownTheme,
    ) -> Vec<Vec<Vec<Vec<Span<'static>>>>> {
        let num_cols = self.rows.first().map(|r| r.len()).unwrap_or(0);
        if num_cols == 0 {
            return vec![];
        }

        // 计算可用宽度（减去边框和间距）
        // 每行: │<space>内容<space>│<space>内容<space>│  → 1 + 3*num_cols 非内容字符
        let border_width = 1 + 3 * num_cols;
        let available_width = max_width.saturating_sub(border_width);

        // 计算每列的最小和理想宽度
        let min_col_widths = self.calculate_min_col_widths(num_cols);
        let ideal_col_widths = self.calculate_ideal_col_widths(num_cols);

        // 如果总宽度超过可用宽度，按比例缩小
        let total_ideal: usize = ideal_col_widths.iter().sum();
        let col_widths = if total_ideal > available_width {
            self.distribute_col_widths(&min_col_widths, &ideal_col_widths, available_width)
        } else {
            ideal_col_widths
        };

        // 包装每个单元格的文本
        let mut wrapped_rows = Vec::new();
        for row in &self.rows {
            let mut wrapped_row = Vec::new();
            for (col_idx, cell) in row.iter().enumerate() {
                let col_width = col_widths.get(col_idx).copied().unwrap_or(0);
                let wrapped = self.wrap_cell_text(cell, col_width);
                wrapped_row.push(wrapped);
            }
            wrapped_rows.push(wrapped_row);
        }

        wrapped_rows
    }

    /// 计算每列的最小宽度（基于最短内容）
    fn calculate_min_col_widths(&self, num_cols: usize) -> Vec<usize> {
        let mut min_widths = vec![0usize; num_cols];
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < num_cols {
                    let w: usize = cell.iter().map(|s| s.content.width()).sum();
                    min_widths[i] = min_widths[i].max(w.min(10)); // 最小宽度至少为10
                }
            }
        }
        min_widths
    }

    /// 计算每列的理想宽度（基于内容长度）
    fn calculate_ideal_col_widths(&self, num_cols: usize) -> Vec<usize> {
        let mut ideal_widths = vec![0usize; num_cols];
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < num_cols {
                    let w: usize = cell.iter().map(|s| s.content.width()).sum();
                    ideal_widths[i] = ideal_widths[i].max(w);
                }
            }
        }
        ideal_widths
    }

    /// 保证最小宽度后按比例分配剩余空间
    ///
    /// 每列先拿到最小宽度（`calculate_min_col_widths` 的值，上限 10），
    /// 剩余空间按 `ideal - min` 的比例分配。这保证 CJK 列至少
    /// 有 ~4-10 显示列宽，不会出现每行仅 1-2 个中文字的情况。
    fn distribute_col_widths(
        &self,
        min_widths: &[usize],
        ideal_widths: &[usize],
        available_width: usize,
    ) -> Vec<usize> {
        let n = ideal_widths.len();
        let min_sum: usize = min_widths.iter().sum();

        // 如果最小宽度之和已超可用宽度，按比例从最小值压缩
        if min_sum >= available_width {
            return min_widths
                .iter()
                .map(|&m| {
                    let scaled = (m as f64 * available_width as f64 / min_sum as f64) as usize;
                    scaled.max(2) // 至少保证 2 列宽，防零宽列
                })
                .collect();
        }

        let mut remaining = available_width - min_sum;

        // 计算各列需要的额外宽度：ideal - min（理想超过最小的部分）
        let extras: Vec<usize> = ideal_widths
            .iter()
            .zip(min_widths.iter())
            .map(|(&ideal, &min)| ideal.saturating_sub(min))
            .collect();
        let total_extra: usize = extras.iter().sum();

        let mut widths = Vec::with_capacity(n);
        for (i, (&min, &extra)) in min_widths.iter().zip(extras.iter()).enumerate() {
            if i == n - 1 {
                // 最后一列取剩余
                widths.push(min + remaining);
            } else {
                let extra_share = (extra * remaining).checked_div(total_extra).unwrap_or(0);
                widths.push(min + extra_share);
                remaining = remaining.saturating_sub(extra_share);
            }
        }

        widths
    }
}
