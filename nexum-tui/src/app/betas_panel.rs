use std::any::Any;

use ratatui::{layout::Rect, Frame};
use tui_textarea::Input;

use super::{
    panel_component::PanelComponent,
    panel_manager::{EventResult, PanelContext, PanelKind},
    App,
};

/// Beta 功能条目定义
#[derive(Debug, Clone)]
pub struct BetaEntry {
    pub key: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
}

/// Beta 功能开关键值
const BETA_KEYS: &[&str] = &[];

/// /betas 面板状态
#[derive(Debug, Clone)]
pub struct BetasPanel {
    /// 所有 beta 条目
    pub entries: Vec<BetaEntry>,
    /// 当前光标索引
    pub cursor: usize,
}

impl BetasPanel {
    /// 从 NexumConfig 构建 BetasPanel
    pub fn from_config(_cfg: &crate::config::NexumConfig) -> Self {
        let entries = BETA_KEYS
            .iter()
            .map(|&key| BetaEntry {
                key: key.to_string(),
                label: key.to_string(),
                description: String::new(),
                enabled: false,
            })
            .collect();

        Self { entries, cursor: 0 }
    }

    /// 切换当前光标处的 beta 开关
    pub fn toggle_current(&mut self) {
        if let Some(entry) = self.entries.get_mut(self.cursor) {
            entry.enabled = !entry.enabled;
        }
    }

    /// 将面板状态应用到 NexumConfig
    pub fn apply_to_config(&self, _cfg: &mut crate::config::NexumConfig) {
        // 当前无活跃 beta 功能，无配置可应用
    }
}

// ─── PanelComponent 实现 ──────────────────────────────────────────────────────

impl PanelComponent for BetasPanel {
    fn kind(&self) -> PanelKind {
        PanelKind::Betas
    }

    fn handle_key(&mut self, input: Input, _ctx: &mut PanelContext<'_>) -> EventResult {
        use tui_textarea::Key;
        match input {
            Input { key: Key::Up, .. } => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                EventResult::Consumed
            }
            Input { key: Key::Down, .. } => {
                if self.cursor < self.entries.len().saturating_sub(1) {
                    self.cursor += 1;
                }
                EventResult::Consumed
            }
            Input {
                key: Key::Left,
                ctrl: false,
                ..
            }
            | Input {
                key: Key::Right,
                ctrl: false,
                ..
            }
            | Input {
                key: Key::Char(' '),
                ctrl: false,
                ..
            } => {
                self.toggle_current();
                EventResult::Consumed
            }
            Input { key: Key::Esc, .. } => EventResult::ClosePanel,
            _ => EventResult::Consumed,
        }
    }

    fn desired_height(&self, _screen_height: u16, _screen_width: u16) -> u16 {
        // 提示行(1) + 空行(1) + 每条目2行 + 边框(2)
        (self.entries.len() as u16 * 2 + 4).max(6)
    }

    fn render(&mut self, f: &mut Frame, app: &mut App, area: Rect) {
        crate::ui::main_ui::panels::betas::render_betas_panel(f, self, app, area);
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn status_bar_hints(&self, _lc: &crate::i18n::LcRegistry) -> Vec<(String, String)> {
        vec![
            (
                "\u{2191}\u{2193}".to_string(),
                "\u{9009}\u{62e9}".to_string(),
            ),
            (
                "\u{2190}\u{2192}".to_string(),
                "\u{5207}\u{6362}".to_string(),
            ),
            ("Esc".to_string(), "\u{5173}\u{95ed}".to_string()),
        ]
    }
}
