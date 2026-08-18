// render_state.rs —— Markdown 渲染状态机的入口模块（facade）
//
// 本文件原为 747 行的 god file，现按 Layered + Module-per-Feature 模式拆分：
//   - coordinator.rs：RenderState 协调器（事件路由 + spans 缓冲 + 生命周期 + 列表类型）
//   - table/ 子模块：表格构建（builder）/ 列宽分配（layout）/ CJK 换行（wrap）/ 渲染（render）
//
// 拆分原则：
//   1. pub API 签名零改动（new/with_max_width/handle_event/flush_line/push_span/lines/current_spans）
//   2. 所有 [TRAP] 注释与不变量注释原样迁移，禁止"简化"
//   3. 生命周期参数 'a 仍挂在 RenderState 上，子模块通过 &self.theme 临时借用

mod coordinator;
mod table;

// 对外仅暴露 RenderState（pub(in crate::markdown) 给父 markdown 模块使用）
pub(in crate::markdown) use coordinator::RenderState;
