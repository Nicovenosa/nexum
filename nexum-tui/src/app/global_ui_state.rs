//! App 级 UI 状态：跨 session 共享的全局 UI 临时状态

use std::{cell::Cell, time::Instant};

use super::{oauth_prompt::OAuthPrompt, setup_wizard::SetupWizardPanel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    Build,
    Plan,
    Think,
    Review,
    Research,
}

impl AgentMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Build => "Build",
            Self::Plan => "Plan",
            Self::Think => "Think",
            Self::Review => "Review",
            Self::Research => "Research",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Build => Self::Plan,
            Self::Plan => Self::Think,
            Self::Think => Self::Review,
            Self::Review => Self::Research,
            Self::Research => Self::Build,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Build => Self::Research,
            Self::Plan => Self::Build,
            Self::Think => Self::Plan,
            Self::Review => Self::Think,
            Self::Research => Self::Review,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDisplayMode {
    Manual,
    Partial,
    Auto,
}

impl ApprovalDisplayMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::Partial => "Parcial",
            Self::Auto => "Auto",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Manual => Self::Partial,
            Self::Partial => Self::Auto,
            Self::Auto => Self::Manual,
        }
    }
}

/// App 级 UI 状态：跨 session 共享的全局 UI 临时状态。
///
/// 与 `ServiceRegistry` 中的"服务"字段（config、MCP pool、cron 等）不同，
/// 这里的字段纯粹是 UI 层面的临时状态（高亮计时、弹窗、鼠标探测等）。
pub struct GlobalUiState {
    /// Estado UI del MemoryGateway (propuesta/conflicto pendientes + toggle).
    pub memory_gw: crate::memory_gateway::MemoryUiState,
    pub setup_wizard: Option<SetupWizardPanel>,
    pub oauth_prompt: Option<OAuthPrompt>,
    pub mode_highlight_until: Option<Instant>,
    pub model_highlight_until: Option<Instant>,
    pub provider_highlight_until: Option<Instant>,
    pub mcp_ready_shown_until: Cell<Option<Instant>>,
    /// MCP 失败提示自动消失计时器（首次显示后 10 秒消失）
    pub mcp_failed_shown_until: Cell<Option<Instant>>,
    pub quit_pending_since: Option<Instant>,
    /// 双击 ESC 检测时间戳（rewind 弹窗触发）
    pub rewind_pending_since: Option<Instant>,
    /// 运行中按 ESC 的 rewind 提示截止时间
    pub rewind_busy_hint_until: Option<Instant>,
    pub quit_requested: bool,
    pub mouse_available: Option<bool>,
    /// Visual-only tick for Nexum motion. It advances from wall-clock timing,
    /// not from input/mouse event loop iterations.
    pub visual_tick: Cell<u64>,
    pub agent_mode: AgentMode,
    /// UI-only approval display. Does not grant permissions or auto-approve tools.
    pub approval_display_mode: ApprovalDisplayMode,
    pub agent_mode_highlight_until: Option<Instant>,
    pub approval_highlight_until: Option<Instant>,
    /// Modo selección de texto (sprint copy-parcial 2026-07-07): cuando está
    /// ON, `run_app` desactiva el mouse capture del terminal para que la
    /// selección nativa de Konsole/tmux/etc. funcione (arrastrar con el mouse
    /// selecciona texto real que se copia con el atajo de la terminal). Cuando
    /// vuelve a OFF, se re-activa el capture y el scroll interno de la TUI
    /// vuelve a andar. Se togglea con Ctrl+S. `run_app` compara este flag con
    /// el estado del capture ya aplicado y reconcilia.
    pub selection_mode: bool,
}

impl Default for GlobalUiState {
    fn default() -> Self {
        Self::new()
    }
}
impl GlobalUiState {
    pub fn new() -> Self {
        Self {
            memory_gw: Default::default(),
            setup_wizard: None,
            oauth_prompt: None,
            mode_highlight_until: None,
            model_highlight_until: None,
            provider_highlight_until: None,
            mcp_ready_shown_until: Cell::new(None),
            mcp_failed_shown_until: Cell::new(None),
            quit_pending_since: None,
            rewind_pending_since: None,
            rewind_busy_hint_until: None,
            quit_requested: false,
            mouse_available: None,
            visual_tick: Cell::new(0),
            agent_mode: AgentMode::Build,
            approval_display_mode: ApprovalDisplayMode::Manual,
            agent_mode_highlight_until: None,
            approval_highlight_until: None,
            selection_mode: false,
        }
    }
}
