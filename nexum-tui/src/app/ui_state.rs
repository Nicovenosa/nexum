use nexum_widgets::ScrollbarMetrics;
use tui_textarea::TextArea;

use super::at_mention::AtMentionState;
use super::hint_ops::SlashHintState;
use crate::app::text_selection::{PanelTextSelection, TextSelection};

/// Record of a textarea mutation for debugging (NEXUM_INPUT_DEBUG=1).
#[derive(Debug, Clone)]
pub struct InputMutation {
    pub source: InputMutationSource,
    pub len_before: usize,
    pub len_after: usize,
    pub timestamp: std::time::Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputMutationSource {
    UserKey,
    Paste,
    Delete,
    History,
    AutocompleteAccepted,
    ProgrammaticClear,
    CommandInsert,
    InterruptedRestore,
    RewindRestore,
    PredictionAccept,
    Unknown,
}

/// 预测输入状态：agent 完成后 LLM 生成的下一步输入建议。
pub struct PredictionState {
    pub text: String,
    pub received_at: std::time::Instant,
}

/// Hitbox del botón "📋 Copiar" en coordenadas de PANTALLA,
/// registrado por frame en render_messages (message_area.rs) y consumido
/// por el handler de click (event/mod.rs). `message_idx` indexa
/// view_messages → identifica QUÉ respuesta copia este botón.
#[derive(Debug, Clone, Copy)]
pub struct CopyButtonHitbox {
    pub rect: ratatui::layout::Rect,
    pub message_idx: usize,
}

/// UI 交互状态：会话级的输入、滚动、选区、历史等。
pub struct UiState {
    pub textarea: TextArea<'static>,
    pub loading: bool,
    pub scroll_offset: u16,
    pub scroll_follow: bool,
    pub show_tool_messages: bool,
    pub hint_cursor: Option<usize>,
    pub input_history: Vec<String>,
    pub history_index: Option<usize>,
    pub draft_input: Option<String>,
    pub text_selection: TextSelection,
    pub messages_area: Option<ratatui::layout::Rect>,
    /// Hitboxes de los botones "📋 Copiar" visibles en el frame
    /// actual. Se limpia y re-registra en cada render (solo botones dentro
    /// del viewport — un botón scrolleado fuera de pantalla no es clickeable).
    pub copy_button_hitboxes: Vec<CopyButtonHitbox>,
    pub textarea_area: Option<ratatui::layout::Rect>,
    pub copy_message_until: Option<std::time::Instant>,
    pub copy_char_count: usize,
    pub panel_selection: PanelTextSelection,
    pub panel_area: Option<ratatui::layout::Rect>,
    pub panel_plain_lines: Vec<String>,
    pub panel_scroll_offset: u16,
    /// 用户是否正在拖拽消息区域右侧滚动条
    pub scrollbar_dragging: bool,
    /// 消息区域滚动条的最大偏移量（内容高度 - 可见高度）
    pub scrollbar_max_offset: u16,
    /// 滚动条拖拽起始时的鼠标 Y 坐标
    pub scrollbar_drag_start_y: u16,
    /// 滚动条拖拽起始时的 scroll offset
    pub scrollbar_drag_start_offset: u16,
    /// Panel scrollbar geometry for mouse interaction
    pub panel_scrollbar_metrics: Option<ScrollbarMetrics>,
    /// Flujo con el que se ruteó el último turno: "DIRECT_CHAT" | "FULL_REACT".
    /// Va al footer para que nunca más se discuta en qué modo estabas.
    pub active_flow: Option<&'static str>,
    /// Whether user is currently dragging the panel scrollbar
    pub panel_scrollbar_dragging: bool,
    /// @ 文件提及状态
    pub at_mention: AtMentionState,
    /// / skill/command 内联补全状态
    pub slash_hint: SlashHintState,
    /// 后台 Agent Bar 光标位置
    pub bg_bar_cursor: Option<usize>,
    /// 后台 Agent Bar 渲染区域（用于鼠标点击检测）
    pub bg_bar_area: Option<ratatui::layout::Rect>,
    /// Write/Edit 工具结果内联 diff 是否可见
    pub diff_visible: bool,
    /// Rewind 完成后待回填到输入框的用户消息文本
    pub pending_rewind_text: Option<String>,
    /// 预测输入建议（灰色 placeholder，Tab 接受）
    pub prediction: Option<PredictionState>,
    /// Escape sequence absorber: two-phase state machine.
    /// Phase 1: `esc_pending` — ESC just seen, waiting for next char.
    ///   If next char is `[` (CSI) or `O` (SS3), transition to phase 2.
    ///   Otherwise, clear pending and let the char through.
    /// Phase 2: `esc_sequence_active` — absorbing until terminator (letter/~).
    /// Prevents Konsole/VT escape-sequence fragments from leaking into textarea.
    pub esc_pending: bool,
    pub esc_sequence_active: bool,
    /// Last mouse Down position (column, row). Used to distinguish isolated
    /// clicks from drag-release for copy-button hit-testing.
    pub mouse_down_pos: Option<(u16, u16)>,
    /// Index of the copy button currently hovered by the mouse (if any).
    /// Updated on MouseEventKind::Moved and consumed by the render thread to
    /// highlight the button in bright white.
    pub hovered_copy_button: Option<usize>,
    /// Debug-only mutation log. When NEXUM_INPUT_DEBUG=1, every textarea
    /// mutation is recorded here for diagnosis of phantom text issues.
    pub input_mutation_log: Vec<InputMutation>,
}

impl UiState {
    /// Log a textarea mutation when NEXUM_INPUT_DEBUG=1 is set.
    /// Tracks source, length before/after, and timestamp.
    pub fn log_mutation(&mut self, source: InputMutationSource, len_before: usize) {
        if std::env::var("NEXUM_INPUT_DEBUG").as_deref() == Ok("1") {
            let len_after: usize = self.textarea.lines().iter().map(|l| l.len()).sum();
            self.input_mutation_log.push(InputMutation {
                source,
                len_before,
                len_after,
                timestamp: std::time::Instant::now(),
            });
            if self.input_mutation_log.len() > 100 {
                self.input_mutation_log
                    .drain(..self.input_mutation_log.len() - 100);
            }
        }
    }

    pub fn new(textarea: TextArea<'static>, cwd: &str, diff_enabled: bool) -> Self {
        let _ = cwd; // 历史路径已迁移至 ~/.peri/，cwd 保留用于未来扩展
        let input_history = super::history_persistence::load_input_history();
        Self {
            textarea,
            loading: false,
            scroll_offset: u16::MAX,
            scroll_follow: true,
            show_tool_messages: false,
            hint_cursor: None,
            input_history,
            history_index: None,
            draft_input: None,
            text_selection: TextSelection::new(),
            messages_area: None,
            copy_button_hitboxes: Vec::new(),
            textarea_area: None,
            copy_message_until: None,
            copy_char_count: 0,
            panel_selection: PanelTextSelection::new(),
            panel_area: None,
            panel_plain_lines: Vec::new(),
            panel_scroll_offset: 0,
            scrollbar_dragging: false,
            scrollbar_max_offset: 0,
            scrollbar_drag_start_y: 0,
            scrollbar_drag_start_offset: 0,
            panel_scrollbar_metrics: None,
            active_flow: None,
            panel_scrollbar_dragging: false,
            at_mention: AtMentionState::new(),
            slash_hint: SlashHintState::default(),
            bg_bar_cursor: None,
            bg_bar_area: None,
            diff_visible: diff_enabled,
            pending_rewind_text: None,
            prediction: None,
            esc_pending: false,
            esc_sequence_active: false,
            mouse_down_pos: None,
            hovered_copy_button: None,
            input_mutation_log: Vec::new(),
        }
    }
}
