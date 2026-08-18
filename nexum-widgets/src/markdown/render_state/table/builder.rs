// table/builder.rs —— TableBuilder 纯数据结构
//
// 仅负责数据收集：rows / current_row / current_cell / alignments / in_head。
// 不含排版/换行/渲染逻辑——这些分别在 layout.rs / wrap.rs / render.rs。
//
// [潜在死字段] in_head 被 TableHead 事件赋值（true/false）但 render_with_wrap 未读取。
//              本轮重构保守保留（不删字段，保持 struct 形状不变），如后续确认无消费方
//              可在 render.rs 显式利用（如 thead 加粗）或删除。详见 god-file-analysis.md
//              #render_state.rs 「dead-code-potential」段。

use pulldown_cmark::Alignment;
use ratatui::text::Span;

pub(in crate::markdown::render_state) type CellContent = Vec<Span<'static>>;

#[derive(Debug, Default)]
pub(in crate::markdown::render_state) struct TableBuilder {
    // 字段对 table/ 子模块内可见（pub(super)=table），其中 current_cell / in_head
    // 需要从 coordinator.rs（render_state 兄弟）访问，因此显式放大到 render_state。
    pub(in crate::markdown::render_state) alignments: Vec<Alignment>,
    pub(in crate::markdown::render_state) rows: Vec<Vec<CellContent>>,
    pub(in crate::markdown::render_state) current_row: Vec<CellContent>,
    pub(in crate::markdown::render_state) current_cell: CellContent,
    pub(in crate::markdown::render_state) in_head: bool,
}

impl TableBuilder {
    pub(in crate::markdown::render_state) fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            ..Default::default()
        }
    }

    pub(in crate::markdown::render_state) fn push_cell(&mut self) {
        let cell = std::mem::take(&mut self.current_cell);
        self.current_row.push(cell);
    }

    pub(in crate::markdown::render_state) fn push_row(&mut self) {
        if !self.current_row.is_empty() {
            let row = std::mem::take(&mut self.current_row);
            self.rows.push(row);
        }
    }
}
