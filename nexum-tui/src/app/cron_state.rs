use std::any::Any;

use nexum_middlewares::cron::{CronControlClient, CronTask};
use ratatui::{
    crossterm::event::{MouseButton, MouseEvent, MouseEventKind},
    layout::Rect,
    Frame,
};
use tui_textarea::Input;

use super::{
    panel_component::PanelComponent,
    panel_list::PanelList,
    panel_manager::{EventResult, PanelContext, PanelKind},
    App,
};

/// CronPanel 面板状态
#[derive(Debug, Clone)]
pub struct CronPanel {
    pub(crate) list: PanelList<CronTask>,
}

impl CronPanel {
    pub fn new(tasks: Vec<CronTask>) -> Self {
        let mut list = PanelList::new();
        list.set_items(tasks);
        Self { list }
    }

    pub fn tasks(&self) -> &[CronTask] {
        self.list.items()
    }

    pub fn cursor(&self) -> usize {
        self.list.cursor()
    }

    pub fn scroll_offset(&self) -> u16 {
        self.list.scroll_offset()
    }
}

impl PanelComponent for CronPanel {
    fn kind(&self) -> PanelKind {
        PanelKind::Cron
    }

    fn handle_key(&mut self, input: Input, _ctx: &mut PanelContext<'_>) -> EventResult {
        use tui_textarea::Key;

        match input {
            Input { key: Key::Up, .. } => {
                self.list.move_cursor(-1);
                EventResult::Consumed
            }
            Input { key: Key::Down, .. } => {
                self.list.move_cursor(1);
                EventResult::Consumed
            }
            Input { key: Key::Esc, .. } => EventResult::ClosePanel,
            _ => EventResult::Consumed,
        }
    }

    fn handle_scroll(&mut self, lines: i16, _ctx: &mut PanelContext<'_>) -> EventResult {
        self.list.handle_scroll(lines, 10);
        EventResult::Consumed
    }

    fn set_scroll_offset(&mut self, offset: u16) {
        self.list.set_scroll_offset(offset);
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        area: Rect,
        _ctx: &mut PanelContext<'_>,
    ) -> EventResult {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            self.list
                .handle_mouse_click(mouse.row, mouse.column, area, 2);
            EventResult::Consumed
        } else {
            EventResult::NotConsumed
        }
    }

    fn desired_height(&self, _screen_height: u16, _screen_width: u16) -> u16 {
        14
    }

    fn render(&mut self, f: &mut Frame, app: &mut App, area: Rect) {
        crate::ui::main_ui::panels::cron::render_cron_panel(f, self, app, area);
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn status_bar_hints(&self, _lc: &crate::i18n::LcRegistry) -> Vec<(String, String)> {
        vec![
            ("\u{2191}\u{2193}".to_string(), _lc.tr("key-move")),
            ("Esc".to_string(), _lc.tr("key-close")),
        ]
    }
}

/// Cron state is a host-provided snapshot for rendering only. It has no local
/// scheduler, timer or trigger receiver.
pub struct CronState {
    pub client: CronControlClient,
    tasks: Vec<CronTask>,
}

impl CronState {
    pub fn new(client: CronControlClient) -> Self {
        Self {
            client,
            tasks: Vec::new(),
        }
    }

    pub fn tasks(&self) -> &[CronTask] {
        &self.tasks
    }

    pub fn set_visible_tasks(&mut self, tasks: Vec<CronTask>) {
        self.tasks = tasks;
    }
}

impl Default for CronState {
    fn default() -> Self {
        Self::new(CronControlClient::unavailable())
    }
}
