// table/mod.rs —— 表格子模块入口
//
// TableBuilder 是自包含的数据结构 + 算法集合：
//   - builder.rs：纯数据结构（rows/current_row/current_cell/alignments/in_head + push_cell/push_row）
//   - layout.rs：列宽分配算法（calculate_min/ideal_col_widths + distribute_col_widths）
//   - wrap.rs：CJK 视觉宽度换行算法 wrap_cell_text + make_border 边框工具（纯算法无状态）
//   - render.rs：渲染（render_with_wrap 边框 + 对齐填充 + 多行单元格高度对齐）
//
// [TRAP] unicode-width 一致性：col_widths 计算（layout.rs）与渲染（render.rs）必须统一用
//        .content.width()，禁止其中一处改用 .len()，否则 CJK 列宽会错位
//        （详见 spec/global/domains/tui.md 与原 god-file-analysis.md#render_state.rs）
// [TRAP] 字符串截断：wrap_cell_text 已正确使用 char_indices + UnicodeWidthChar，
//        重构时禁止改用 &s[..N] 字节切片，否则 CJK 会 panic（CLAUDE.md 明确要求 chars 级操作）

mod builder;
mod layout;
mod render;
mod wrap;

pub(in crate::markdown::render_state) use builder::TableBuilder;
