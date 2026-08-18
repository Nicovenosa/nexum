use std::any::Any;

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    layout::Rect,
    Frame,
};
use tui_textarea::Input;

use super::{
    panel_component::PanelComponent,
    panel_manager::{EventResult, PanelContext, PanelKind},
    App,
};
use crate::config::{NexumConfig, ProviderConfig, ThinkingConfig};

// Models reserved for internal use (Hormiguero low-cost workers) and therefore
// hidden from the user-facing `/modelo` picker. They remain available in Ollama
// for internal callers that read `/v1/models` directly; this filter only governs
// what the user can select as their chat model.
//
// The env var `NEXUM_RESERVED_MODELS` (comma-separated) EXTENDS the default
// list at runtime — it never shrinks it — so the baseline reserved set is
// always enforced. See NEXUM_RESERVED_MODEL_POLICY.md.
const DEFAULT_RESERVED_INTERNAL_MODELS: &[&str] = &["qwen3:0.6b"];

fn reserved_internal_models() -> std::collections::HashSet<String> {
    let mut set: std::collections::HashSet<String> = DEFAULT_RESERVED_INTERNAL_MODELS
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Ok(extra) = std::env::var("NEXUM_RESERVED_MODELS") {
        for m in extra.split(',').map(str::trim).filter(|m| !m.is_empty()) {
            set.insert(m.to_string());
        }
    }
    set
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelChoice {
    pub label: String,
    pub key: String,
    pub provider_id: String,
    pub family: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderSection {
    pub family: String,
    pub provider_id: String,
    pub models: Vec<ModelChoice>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DisplayRow {
    Header {
        family: String,
        provider_id: String,
    },
    Model {
        choice: ModelChoice,
        index_in_section: usize,
    },
}

// ─── AliasTab 枚举 ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AliasTab {
    Opus,
    Sonnet,
    Haiku,
}

impl AliasTab {
    pub fn label(&self) -> &str {
        match self {
            Self::Opus => "Opus",
            Self::Sonnet => "Sonnet",
            Self::Haiku => "Haiku",
        }
    }

    pub fn to_key(&self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Sonnet => "sonnet",
            Self::Haiku => "haiku",
        }
    }

    pub fn description(&self) -> &str {
        match self {
            Self::Opus => "Most capable for complex work",
            Self::Sonnet => "Balanced performance and speed",
            Self::Haiku => "Fastest for quick answers",
        }
    }
}

// ─── 行索引常量 ─────────────────────────────────────────────────────────────────

pub const ROW_OPUS: usize = 0;
pub const ROW_SONNET: usize = 1;
pub const ROW_HAIKU: usize = 2;
pub const ROW_MAX_TOKENS: usize = 3;
pub const ROW_EFFORT: usize = 4;
pub const ROW_1M_CONTEXT: usize = 5;
pub const ROW_COUNT: usize = 6;

// ─── ModelPanel ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ModelPanel {
    pub provider_name: String,
    /// Explicit catalog failure. The model selector never fabricates fallback
    /// rows when its canonical source is missing or corrupt.
    pub catalog_error: Option<String>,
    pub active_tab: AliasTab,
    model_choices: Vec<ModelChoice>,
    display_rows: Vec<DisplayRow>,
    active_model_key: String,
    active_provider_id: String,
    pub buf_thinking_effort: String,
    pub buf_max_tokens: u32,
    pub buf_context_1m: bool,
    pub(crate) cursor: usize,
    /// Primer línea visible del viewport (fix UX popups Bug 1). Lo actualiza
    /// el render con `scroll::ensure_visible` para seguir al cursor.
    pub(crate) scroll_offset: usize,
}

impl ModelPanel {
    pub fn from_config(cfg: &NexumConfig) -> Self {
        Self::from_config_with_openai_env(
            cfg,
            std::env::var("OPENAI_MODELS").ok().as_deref(),
            std::env::var("OPENAI_MODEL").ok().as_deref(),
        )
    }

    pub fn from_config_with_openai_env(
        cfg: &NexumConfig,
        openai_models: Option<&str>,
        openai_model: Option<&str>,
    ) -> Self {
        let active_tab = match cfg.config.active_alias.as_str() {
            "sonnet" => AliasTab::Sonnet,
            "haiku" => AliasTab::Haiku,
            _ => AliasTab::Opus,
        };

        let (model_choices, display_rows, catalog_error) = build_all_model_choices(
            active_provider(cfg),
            openai_models,
            openai_model,
            &cfg.config.active_provider_id,
        );
        let active_model_key = active_model_key_for_provider(
            &model_choices,
            cfg.config.active_alias.as_str(),
            &cfg.config.active_provider_id,
            openai_model,
        );

        // El catálogo manda en el nombre visible: es quien sabe que `opencode_zen`
        // se muestra como OpenCode Free. Pero un provider configurado a mano no
        // está en el catálogo, y quedarse sin nombre es el panel callándose algo
        // que sí sabe. Catálogo primero, config como respaldo.
        let provider_name = catalog_provider_display_name(&cfg.config.active_provider_id)
            .or_else(|| {
                cfg.config
                    .providers
                    .iter()
                    .find(|p| p.id == cfg.config.active_provider_id)
                    .map(|p| p.display_name().to_string())
            })
            .unwrap_or_default();

        let cursor = find_cursor_for_model(&display_rows, &active_model_key)
            .unwrap_or_else(|| first_model_cursor(&display_rows));

        let effort = cfg
            .config
            .thinking
            .as_ref()
            .map(|t| t.effort.clone())
            .unwrap_or_else(|| "high".to_string());

        let max_tokens = cfg
            .config
            .thinking
            .as_ref()
            .map(|t| t.max_tokens)
            .unwrap_or(32000);

        let context_1m = cfg.config.context_1m.unwrap_or(false);

        Self {
            provider_name,
            catalog_error,
            active_tab,
            model_choices,
            display_rows,
            active_model_key,
            active_provider_id: cfg.config.active_provider_id.clone(),
            buf_thinking_effort: effort,
            buf_max_tokens: max_tokens,
            buf_context_1m: context_1m,
            cursor,
            scroll_offset: 0,
        }
    }

    /// Mueve el cursor N pasos seleccionables hacia abajo (dir=+1) o arriba
    /// (dir=-1), salteando headers. Usado por ↑↓/jk (1 paso) y PgUp/PgDn (8).
    pub(crate) fn move_cursor(&mut self, dir: i32, steps: usize) {
        for _ in 0..steps {
            let next = if dir > 0 {
                next_selectable_row(&self.display_rows, self.cursor, self.row_count())
            } else {
                prev_selectable_row(&self.display_rows, self.cursor)
            };
            if next == self.cursor {
                break;
            }
            self.cursor = next;
        }
    }

    /// Cursor al primer ítem seleccionable (g/Home).
    pub(crate) fn cursor_to_first(&mut self) {
        self.cursor = first_model_cursor(&self.display_rows);
    }

    /// Cursor al último ítem seleccionable (G/End) — la fila de 1M context.
    pub(crate) fn cursor_to_last(&mut self) {
        self.cursor = self.context_1m_row();
    }

    /// 光标位置
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn model_choice_labels(&self) -> Vec<String> {
        self.model_choices
            .iter()
            .map(|choice| choice.label.clone())
            .collect()
    }

    pub fn display_rows(&self) -> &[DisplayRow] {
        &self.display_rows
    }

    pub fn display_row_count(&self) -> usize {
        self.display_rows.len()
    }

    pub fn active_model_label(&self) -> String {
        self.model_choices
            .iter()
            .find(|choice| choice.key == self.active_model_key)
            .or_else(|| {
                let legacy_key = self.active_tab.to_key();
                self.model_choices
                    .iter()
                    .find(|choice| choice.key == legacy_key)
            })
            .map(|choice| choice.label.clone())
            .unwrap_or_default()
    }

    pub fn model_choice_count(&self) -> usize {
        self.model_choices.len()
    }

    pub fn model_choice(&self, row: usize) -> Option<&ModelChoice> {
        self.model_choices.get(row)
    }

    pub fn is_model_row(&self, row: usize) -> bool {
        self.display_rows
            .get(row)
            .is_some_and(|r| matches!(r, DisplayRow::Model { .. }))
    }

    pub fn is_model_active(&self, row: usize) -> bool {
        self.display_rows
            .get(row)
            .and_then(|r| match r {
                DisplayRow::Model { choice, .. } => Some(choice.key == self.active_model_key),
                _ => None,
            })
            .unwrap_or(false)
    }

    pub fn max_tokens_row(&self) -> usize {
        self.display_rows.len()
    }

    pub fn effort_row(&self) -> usize {
        self.display_rows.len() + 1
    }

    pub fn context_1m_row(&self) -> usize {
        self.display_rows.len() + 2
    }

    pub fn row_count(&self) -> usize {
        self.display_rows.len() + 3
    }

    fn select_model_row(&mut self, row: usize) {
        let Some(display_row) = self.display_rows.get(row) else {
            return;
        };
        let DisplayRow::Model { choice, .. } = display_row else {
            return;
        };
        self.active_model_key = choice.key.clone();
        self.active_provider_id = choice.provider_id.clone();
        if let Some(tab) = match choice.key.as_str() {
            "opus" => Some(AliasTab::Opus),
            "sonnet" => Some(AliasTab::Sonnet),
            "haiku" => Some(AliasTab::Haiku),
            _ => None,
        } {
            self.active_tab = tab;
        }
    }

    /// 循环切换 effort：low → medium → high → xhigh → max → low（任意光标位置可切换）
    pub fn cycle_effort(&mut self, reverse: bool) {
        if reverse {
            self.buf_thinking_effort = match self.buf_thinking_effort.as_str() {
                "low" => "max".to_string(),
                "max" => "xhigh".to_string(),
                "xhigh" => "high".to_string(),
                "high" => "medium".to_string(),
                _ => "low".to_string(),
            };
        } else {
            self.buf_thinking_effort = match self.buf_thinking_effort.as_str() {
                "low" => "medium".to_string(),
                "medium" => "high".to_string(),
                "high" => "xhigh".to_string(),
                "xhigh" => "max".to_string(),
                _ => "low".to_string(),
            };
        }
    }

    /// max_tokens 预设值：8000 → 16000 → 32000 → 64000 → 128000 → 8000
    const MAX_TOKENS_PRESETS: &[u32] = &[8000, 16000, 32000, 64000, 128000];

    /// 循环切换 max_tokens 预设值
    pub fn cycle_max_tokens(&mut self, reverse: bool) {
        let current = self.buf_max_tokens;
        let presets = Self::MAX_TOKENS_PRESETS;
        if let Some(pos) = presets.iter().position(|&v| v == current) {
            if reverse {
                let next = if pos == 0 { presets.len() - 1 } else { pos - 1 };
                self.buf_max_tokens = presets[next];
            } else {
                let next = (pos + 1) % presets.len();
                self.buf_max_tokens = presets[next];
            }
        } else {
            // 非预设值回退到最近的预设值
            let pos = presets
                .partition_point(|&v| v < current)
                .min(presets.len() - 1);
            if reverse {
                self.buf_max_tokens = presets[pos.saturating_sub(1)];
            } else {
                self.buf_max_tokens = presets[pos];
            }
        }
    }

    /// 将面板状态写入 NexumConfig（alias + thinking + max_tokens + 1M context）
    pub fn apply_to_config(&self, cfg: &mut NexumConfig) {
        cfg.config.active_alias = self.active_model_key.clone();
        cfg.config.active_provider_id = self.active_provider_id.clone();
        let t = cfg.config.thinking.get_or_insert_with(|| ThinkingConfig {
            enabled: true,
            budget_tokens: 8000,
            effort: self.buf_thinking_effort.clone(),
            max_tokens: self.buf_max_tokens,
        });
        t.enabled = true;
        t.effort = self.buf_thinking_effort.clone();
        t.max_tokens = self.buf_max_tokens;
        cfg.config.context_1m = Some(self.buf_context_1m);
    }
}

// ─── PanelComponent 实现 ──────────────────────────────────────────────────────

impl PanelComponent for ModelPanel {
    fn kind(&self) -> PanelKind {
        PanelKind::Model
    }

    fn handle_key(&mut self, input: Input, ctx: &mut PanelContext<'_>) -> EventResult {
        use tui_textarea::Key;
        match input {
            Input { key: Key::Esc, .. } => EventResult::ClosePanel,
            // ↑ / k: ítem anterior (headers se saltan).
            Input { key: Key::Up, .. }
            | Input {
                key: Key::Char('k'),
                ctrl: false,
                alt: false,
                ..
            } => {
                self.move_cursor(-1, 1);
                EventResult::Consumed
            }
            // ↓ / j: siguiente ítem.
            Input { key: Key::Down, .. }
            | Input {
                key: Key::Char('j'),
                ctrl: false,
                alt: false,
                ..
            } => {
                self.move_cursor(1, 1);
                EventResult::Consumed
            }
            // PgDn / Ctrl+D: una página (8 ítems) hacia abajo.
            Input {
                key: Key::PageDown, ..
            }
            | Input {
                key: Key::Char('d'),
                ctrl: true,
                ..
            } => {
                self.move_cursor(1, 8);
                EventResult::Consumed
            }
            // PgUp / Ctrl+U: una página hacia arriba.
            Input {
                key: Key::PageUp, ..
            }
            | Input {
                key: Key::Char('u'),
                ctrl: true,
                ..
            } => {
                self.move_cursor(-1, 8);
                EventResult::Consumed
            }
            // g / Home: primer ítem.
            Input { key: Key::Home, .. }
            | Input {
                key: Key::Char('g'),
                ctrl: false,
                alt: false,
                ..
            } => {
                self.cursor_to_first();
                EventResult::Consumed
            }
            // G / End: último ítem.
            Input { key: Key::End, .. }
            | Input {
                key: Key::Char('G'),
                ctrl: false,
                alt: false,
                ..
            } => {
                self.cursor_to_last();
                EventResult::Consumed
            }
            Input {
                key: Key::Enter, ..
            } => match self.cursor() {
                row if self.is_model_row(row) => {
                    self.select_model_row(row);
                    Self::apply_and_close(self, ctx);
                    EventResult::ClosePanel
                }
                row if row == self.effort_row() => {
                    self.cycle_effort(false);
                    EventResult::Consumed
                }
                row if row == self.max_tokens_row() => {
                    self.cycle_max_tokens(false);
                    EventResult::Consumed
                }
                row if row == self.context_1m_row() => {
                    self.buf_context_1m = !self.buf_context_1m;
                    ModelPanel::apply_1m_context(self, ctx);
                    EventResult::Consumed
                }
                _ => EventResult::Consumed,
            },
            // Space: 切换 effort 等级（无需选中 effort 行）或 max_tokens 或 1M 上下文
            Input {
                key: Key::Char(' '),
                ..
            } => {
                if self.is_model_row(self.cursor()) {
                    self.select_model_row(self.cursor());
                    Self::apply_and_close(self, ctx);
                    EventResult::ClosePanel
                } else if self.cursor() == self.max_tokens_row() {
                    self.cycle_max_tokens(false);
                    EventResult::Consumed
                } else if self.cursor() == self.context_1m_row() {
                    self.buf_context_1m = !self.buf_context_1m;
                    ModelPanel::apply_1m_context(self, ctx);
                    EventResult::Consumed
                } else {
                    self.cycle_effort(false);
                    EventResult::Consumed
                }
            }
            // ←/→: 随时切换 effort 等级或 max_tokens 或 1M 上下文
            Input { key: Key::Left, .. } => {
                if self.cursor() == self.max_tokens_row() {
                    self.cycle_max_tokens(true);
                    EventResult::Consumed
                } else if self.cursor() == self.context_1m_row() {
                    self.buf_context_1m = !self.buf_context_1m;
                    ModelPanel::apply_1m_context(self, ctx);
                    EventResult::Consumed
                } else {
                    self.cycle_effort(true);
                    EventResult::Consumed
                }
            }
            Input {
                key: Key::Right, ..
            } => {
                if self.cursor() == self.max_tokens_row() {
                    self.cycle_max_tokens(false);
                    EventResult::Consumed
                } else if self.cursor() == self.context_1m_row() {
                    self.buf_context_1m = !self.buf_context_1m;
                    ModelPanel::apply_1m_context(self, ctx);
                    EventResult::Consumed
                } else {
                    self.cycle_effort(false);
                    EventResult::Consumed
                }
            }
            _ => EventResult::Consumed,
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        area: Rect,
        ctx: &mut PanelContext<'_>,
    ) -> EventResult {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            // border_top=1，计算点击的行索引
            let relative_y = mouse.row.saturating_sub(area.y);
            if relative_y >= 1 {
                let clicked = (relative_y - 1) as usize;
                if clicked < self.row_count() {
                    self.cursor = clicked;
                    return self.handle_key(
                        Input::from(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                        ctx,
                    );
                }
            }
        }
        EventResult::NotConsumed
    }

    fn desired_height(&self, _screen_height: u16, _screen_width: u16) -> u16 {
        // Además de las filas, el render mete una línea en blanco antes de
        // cada header de sección (menos el primero); sin contarlas, la última
        // fila de config quedaba fuera del viewport (Sprint C).
        let section_blanks = self
            .display_rows
            .iter()
            .filter(|r| matches!(r, DisplayRow::Header { .. }))
            .count()
            .saturating_sub(1) as u16;
        self.row_count() as u16 + section_blanks + 7
    }

    fn render(&mut self, f: &mut Frame, app: &mut App, area: Rect) {
        crate::ui::main_ui::panels::model::render_model_panel(f, self, app, area);
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn status_bar_hints(&self, _lc: &crate::i18n::LcRegistry) -> Vec<(String, String)> {
        vec![
            ("↑↓".to_string(), _lc.tr("key-move")),
            ("Enter".to_string(), _lc.tr("key-confirm")),
            ("←→/Space".to_string(), _lc.tr("key-effort")),
            ("Esc".to_string(), _lc.tr("key-close")),
        ]
    }
}

impl ModelPanel {
    /// 将面板状态写入 config，更新 provider/model 名称。
    ///
    /// Fix UX popups (Bug 3): NO se pushea system note de "modelo cambiado" —
    /// eso llenaba `view_messages` y sacaba al usuario del splash. El feedback
    /// es la statusbar (provider · modelo) que cambia al instante.
    fn apply_and_close(panel: &ModelPanel, ctx: &mut PanelContext<'_>) {
        let cfg_arc = ctx.services.nexum_config.clone();
        let mut cfg = cfg_arc.write();
        let mut candidate = cfg.clone();
        panel.apply_to_config(&mut candidate);
        if let Err(error) = nexum_acp::provider::routes::validate_installed_selection(
            &candidate.config.active_provider_id,
            &candidate.config.active_alias,
        ) {
            ctx.session_mgr
                .current_mut()
                .messages
                .push_system_note(error.to_string());
            return;
        }

        // Provider + model are one atomic execution route. If activation
        // fails, preserve the previous pair; otherwise the next submit would
        // turn a precise resolver failure into the unrelated generic
        // "No API key" message.
        if let Err(msg) =
            ensure_active_provider_config(&mut candidate, &panel.active_provider_id, panel)
        {
            ctx.session_mgr
                .current_mut()
                .messages
                .push_system_note(provider_activation_error(
                    &panel.active_provider_id,
                    &candidate.config.active_alias,
                    &msg,
                ));
            return;
        }
        *cfg = candidate;

        if let Err(e) = App::save_config(&cfg, ctx.services.config_path_override.as_deref()) {
            ctx.session_mgr
                .current_mut()
                .messages
                .push_system_note(ctx.services.lc.tr_args(
                    "app-config-save-failed",
                    &[("error".into(), e.to_string().into())],
                ));
        }

        if let Some(p) = crate::app::agent::LlmProvider::from_config(&cfg) {
            // Statusbar: preferir el nombre configurado del provider
            // ("OpenCode Zen") sobre el genérico de LlmProvider ("OpenAI").
            ctx.services.provider_name =
                provider_display_name(&cfg).unwrap_or_else(|| p.display_name().to_string());
            ctx.services.model_name = p.model_name().to_string();

            // 同步 context_window 到 TUI 状态（agent.context_window 用于 status line 显示）
            let mut cw = p.context_window();
            if panel.buf_context_1m {
                cw = 1_000_000;
            }
            if cw > 0 {
                ctx.session_mgr.current_mut().agent.context_window = cw;
            }
        }

        // 通过 ACP 协议同步到 Server
        let alias = cfg.config.active_alias.clone();
        let active_pid = cfg.config.active_provider_id.clone();
        let config_for_acp = cfg.clone();
        drop(cfg);
        // La selección debe sobrevivir reinicios aunque el workspace settings
        // pise los campos globales al cargar.
        patch_workspace_active(&active_pid, &alias);
        if let Some(ref acp_client) = ctx.acp_client {
            let acp = acp_client.clone();
            let config = config_for_acp;
            let effort = panel.buf_thinking_effort.clone();
            let context_1m_val = panel.buf_context_1m.to_string();
            tokio::spawn(async move {
                // Full config synchronization is required: sending only the
                // model preserves the host's previous provider/endpoint and
                // caused MiMo models to be sent to the Codex bridge.
                if let Err(error) = acp.update_config(&config).await {
                    acp.report_turn_failure(format!(
                        "PROVIDER_ACTIVATION_FAILED: provider_id={} model_id={}: {}",
                        config.config.active_provider_id,
                        config.config.active_alias,
                        error.message
                    ));
                    return;
                }
                let _ = acp.set_config_option("thinking_effort", &effort).await;
                let _ = acp.set_config_option("context_1m", &context_1m_val).await;
            });
        }
    }

    /// 即时应用 1M 上下文开关（不关闭面板）
    fn apply_1m_context(panel: &ModelPanel, ctx: &mut PanelContext<'_>) {
        let cfg_arc = ctx.services.nexum_config.clone();
        let mut cfg = cfg_arc.write();
        cfg.config.context_1m = Some(panel.buf_context_1m);

        // Fix UX popups (Bug 3): sin system note informativo — el estado ON/OFF
        // ya se ve en la fila del panel; el note sacaba al usuario del splash.

        if let Err(e) = App::save_config(&cfg, ctx.services.config_path_override.as_deref()) {
            ctx.session_mgr
                .current_mut()
                .messages
                .push_system_note(ctx.services.lc.tr_args(
                    "app-config-save-failed",
                    &[("error".into(), e.to_string().into())],
                ));
        }

        // 同步 context_window 到 TUI 状态
        if let Some(p) = crate::app::agent::LlmProvider::from_config(&cfg) {
            let mut cw = p.context_window();
            if panel.buf_context_1m {
                cw = 1_000_000;
            }
            if cw > 0 {
                ctx.session_mgr.current_mut().agent.context_window = cw;
            }
        }

        // 通过 ACP 协议同步到 Server
        if let Some(ref acp_client) = ctx.acp_client {
            let acp = acp_client.clone();
            let val = panel.buf_context_1m.to_string();
            tokio::spawn(async move {
                let _ = acp.set_config_option("context_1m", &val).await;
            });
        }
    }
}

fn active_provider(cfg: &NexumConfig) -> Option<&ProviderConfig> {
    cfg.config
        .providers
        .iter()
        .find(|p| p.id == cfg.config.active_provider_id)
}

fn build_all_model_choices(
    _provider: Option<&ProviderConfig>,
    _openai_models: Option<&str>,
    _openai_model: Option<&str>,
    _active_provider_id: &str,
) -> (Vec<ModelChoice>, Vec<DisplayRow>, Option<String>) {
    match build_all_model_choices_from_catalog() {
        Ok((choices, rows)) => (choices, rows, None),
        Err(error) => (Vec::new(), Vec::new(), Some(error)),
    }
}

fn build_all_model_choices_from_catalog() -> Result<(Vec<ModelChoice>, Vec<DisplayRow>), String> {
    let (doc, _) =
        super::provider_panel::load_catalog_document().map_err(|error| error.to_string())?;
    build_choices_from_doc(&doc)
}

/// Costura de test sobre el camino tipado.
///
/// La carga real va por `load_catalog_document`, que devuelve `CatalogLoadError`
/// en vez de un picker vacío fabricado. Pero leer del disco hace que el
/// resultado dependa de qué providers tenga conectado quien corre los tests.
/// Estas dos funciones dejan inyectar el documento sin volver a la lectura laxa
/// por `serde_json::Value` con `.ok()` encadenado.
pub(crate) fn build_model_choices_from_catalog_value(
    value: &serde_json::Value,
) -> Option<(Vec<ModelChoice>, Vec<DisplayRow>)> {
    let doc: super::provider_panel::CatalogDoc = serde_json::from_value(value.clone()).ok()?;
    build_choices_from_doc(&doc).ok()
}

/// El catálogo es la única autoridad de modelos seleccionables. Un catálogo
/// ausente, inválido o sin providers usables da un picker vacío a propósito.
pub(crate) fn catalog_model_choices(
    value: Option<&serde_json::Value>,
) -> (Vec<ModelChoice>, Vec<DisplayRow>) {
    value
        .and_then(build_model_choices_from_catalog_value)
        .unwrap_or_default()
}

fn build_choices_from_doc(
    doc: &super::provider_panel::CatalogDoc,
) -> Result<(Vec<ModelChoice>, Vec<DisplayRow>), String> {
    let reserved = reserved_internal_models();

    let mut all_choices: Vec<ModelChoice> = Vec::new();
    let mut sections: Vec<ProviderSection> = Vec::new();

    for provider in &doc.providers {
        // El picker no ofrece lo que no puede correr. La carga y los accesores
        // tipados vienen del lado que endureció el catálogo; este filtro viene
        // del lado que sacó los modelos fabricados del selector. Sin él, un
        // provider sin credencial usable igual aparecería con sus modelos y el
        // turno fallaría recién al enviarlo.
        if !provider.is_usable() {
            continue;
        }
        let pid = provider.stable_id();
        let family = provider.name();
        let models: Vec<String> = provider
            .user_facing_models()
            .iter()
            .filter(|model| !reserved.contains(model.as_str()))
            .cloned()
            .collect();

        if models.is_empty() {
            continue;
        }

        let section_choices: Vec<ModelChoice> = models
            .into_iter()
            .map(|model| ModelChoice {
                label: model.clone(),
                key: model,
                provider_id: pid.to_string(),
                family: family.to_string(),
            })
            .collect();

        sections.push(ProviderSection {
            family: family.to_string(),
            provider_id: pid.to_string(),
            models: section_choices.clone(),
        });
        all_choices.extend(section_choices);
    }

    if all_choices.is_empty() {
        return Err("Provider catalog válido pero sin modelos seleccionables".to_string());
    }

    let display_rows = build_display_rows_from_sections(&sections);
    Ok((all_choices, display_rows))
}

fn build_display_rows_from_sections(sections: &[ProviderSection]) -> Vec<DisplayRow> {
    let mut rows = Vec::new();
    for section in sections {
        rows.push(DisplayRow::Header {
            family: section.family.clone(),
            provider_id: section.provider_id.clone(),
        });
        for (idx, choice) in section.models.iter().enumerate() {
            rows.push(DisplayRow::Model {
                choice: choice.clone(),
                index_in_section: idx,
            });
        }
    }
    rows
}

fn build_display_rows_from_choices(choices: &[ModelChoice]) -> Vec<DisplayRow> {
    if choices.is_empty() {
        return Vec::new();
    }
    let family = choices[0].family.clone();
    let provider_id = choices[0].provider_id.clone();
    let mut rows = vec![DisplayRow::Header {
        family: family.clone(),
        provider_id: provider_id.clone(),
    }];
    for (idx, choice) in choices.iter().enumerate() {
        rows.push(DisplayRow::Model {
            choice: choice.clone(),
            index_in_section: idx,
        });
    }
    rows
}

fn find_cursor_for_model(display_rows: &[DisplayRow], model_key: &str) -> Option<usize> {
    display_rows.iter().position(|r| match r {
        DisplayRow::Model { choice, .. } => choice.key == model_key,
        _ => false,
    })
}

fn first_model_cursor(display_rows: &[DisplayRow]) -> usize {
    display_rows
        .iter()
        .position(|r| matches!(r, DisplayRow::Model { .. }))
        .unwrap_or(0)
}

fn next_selectable_row(display_rows: &[DisplayRow], current: usize, max_row: usize) -> usize {
    let mut next = current + 1;
    while next < max_row {
        if next >= display_rows.len() {
            return next;
        }
        if matches!(display_rows.get(next), Some(DisplayRow::Model { .. })) {
            return next;
        }
        next += 1;
    }
    current
}

fn prev_selectable_row(display_rows: &[DisplayRow], current: usize) -> usize {
    if current == 0 {
        return 0;
    }
    let mut prev = current - 1;
    loop {
        if prev == 0 {
            return if matches!(display_rows.first(), Some(DisplayRow::Model { .. })) {
                0
            } else {
                current
            };
        }
        if matches!(display_rows.get(prev), Some(DisplayRow::Model { .. })) {
            return prev;
        }
        prev -= 1;
    }
}

// ─── Provider credential resolution (fix UX popups Bug 2) ────────────────────

/// Credenciales locales resueltas para un provider (via provider_resolve.py).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedProvider {
    pub provider_id: String,
    pub display_name: String,
    pub base_url: String,
    pub api_key: String,
    pub protocol: String,
}

/// Nombre visible del provider activo según la config (p.ej. "OpenCode Zen"),
/// para la statusbar — en lugar del genérico de LlmProvider ("OpenAI").
pub(crate) fn provider_display_name(cfg: &NexumConfig) -> Option<String> {
    let (doc, _) = super::provider_panel::load_catalog_document().ok()?;
    let provider = doc.provider(&cfg.config.active_provider_id)?;
    provider
        .user_facing_models()
        .iter()
        .any(|model| model == &cfg.config.active_alias)
        .then(|| provider.name().to_string())
}

fn catalog_provider_display_name(provider_id: &str) -> Option<String> {
    let (doc, _) = super::provider_panel::load_catalog_document().ok()?;
    doc.provider(provider_id)
        .map(|provider| provider.name().to_string())
}

/// Persists a credential result produced by the explicit login flow.
pub(crate) fn upsert_resolved_provider(cfg: &mut NexumConfig, r: &ResolvedProvider) {
    let provider_type = if r.protocol == "anthropic" {
        "anthropic"
    } else {
        "openai"
    };
    if let Some(provider) = cfg
        .config
        .providers
        .iter_mut()
        .find(|provider| provider.id == r.provider_id)
    {
        provider.provider_type = provider_type.to_string();
        provider.api_key = r.api_key.clone();
        provider.base_url = r.base_url.clone();
        if provider.name.is_none() {
            provider.name = Some(r.display_name.clone());
        }
    } else {
        cfg.config.providers.push(ProviderConfig {
            id: r.provider_id.clone(),
            provider_type: provider_type.to_string(),
            api_key: r.api_key.clone(),
            base_url: r.base_url.clone(),
            name: Some(r.display_name.clone()),
            ..Default::default()
        });
    }
}

/// Mirrors an explicit login into an existing workspace override.
pub(crate) fn patch_workspace_provider(r: &ResolvedProvider) {
    let Some(workspace_path) = crate::config::workspace_config_path() else {
        return;
    };
    let Ok(raw) = std::fs::read_to_string(&workspace_path) else {
        return;
    };
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let provider_type = if r.protocol == "anthropic" {
        "anthropic"
    } else {
        "openai"
    };
    let entry = serde_json::json!({
        "id": r.provider_id,
        "type": provider_type,
        "apiKey": r.api_key,
        "baseUrl": r.base_url,
        "name": r.display_name,
    });
    let Some(providers) = doc
        .pointer_mut("/config/providers")
        .and_then(|value| value.as_array_mut())
    else {
        return;
    };
    if let Some(existing) = providers
        .iter_mut()
        .find(|provider| provider.get("id").and_then(|value| value.as_str()) == Some(r.provider_id.as_str()))
    {
        *existing = entry;
    } else {
        providers.push(entry);
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&doc) {
        let temporary = workspace_path.with_extension("json.tmp");
        if std::fs::write(&temporary, serialized).is_ok() {
            let _ = std::fs::rename(temporary, workspace_path);
        }
    }
}

/// Persiste la selección activa (provider + alias) también en el workspace
/// settings: sus campos no vacíos pisan a los globales al cargar, así que sin
/// esto la selección de /modelo no sobrevive un reinicio.
pub(crate) fn patch_workspace_active(provider_id: &str, alias: &str) {
    let Some(ws_path) = crate::config::workspace_config_path() else {
        return;
    };
    let Ok(raw) = std::fs::read_to_string(&ws_path) else {
        return;
    };
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(config) = doc.pointer_mut("/config").and_then(|v| v.as_object_mut()) else {
        return;
    };
    let had_active = config.contains_key("active_provider_id");
    let had_alias = config.contains_key("active_alias");
    if !had_active && !had_alias {
        return; // el workspace no pisa la selección: nada que actualizar
    }
    if had_active {
        config.insert(
            "active_provider_id".to_string(),
            serde_json::Value::String(provider_id.to_string()),
        );
    }
    if had_alias {
        config.insert(
            "active_alias".to_string(),
            serde_json::Value::String(alias.to_string()),
        );
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&doc) {
        let tmp = ws_path.with_extension("json.tmp");
        if std::fs::write(&tmp, serialized).is_ok() {
            let _ = std::fs::rename(&tmp, &ws_path);
        }
    }
}

fn active_model_key_for_provider(
    choices: &[ModelChoice],
    active_alias: &str,
    active_provider_id: &str,
    openai_model: Option<&str>,
) -> String {
    if let Some(choice) = choices.iter().find(|c| c.key == active_alias) {
        return choice.key.clone();
    }
    if let Some(model) = openai_model {
        if let Some(choice) = choices.iter().find(|c| c.key == model) {
            return choice.key.clone();
        }
    }
    if let Some(choice) = choices.iter().find(|c| c.provider_id == active_provider_id) {
        return choice.key.clone();
    }
    choices
        .first()
        .map(|c| c.key.clone())
        .unwrap_or_default()
}


/// Garantiza que el provider activo tenga ProviderConfig con credenciales.
/// Si falta (provider recién elegido desde el catálogo en /modelo), resuelve
/// las credenciales locales mediante el resolver versionado del slot.
///
/// Devuelve Err(mensaje) si no se pudo resolver — en ese caso el runtime
/// sigue en el provider anterior y el caller informa el error.
/// Endpoint declarado por el route registry para un provider.
///
/// El registry es la fuente del endpoint —lo estampa `nexum-package` y Doctor
/// lo valida contra el catálogo— así que la activación no tiene por qué
/// adivinarlo ni depender de que el resolver de credenciales lo devuelva.
/// Las rutas `cliproxyapi://` se omiten a propósito: ésas SÍ van por el puente
/// y no llevan base_url propia.
fn endpoint_del_registry(provider_id: &str) -> Option<String> {
    let (registry, _) =
        nexum_acp::provider::routes::ProviderRouteRegistry::load_installed().ok()?;
    let normalizado = provider_id.replace('-', "_");
    registry
        .routes
        .iter()
        .find(|r| r.provider_id.replace('-', "_") == normalizado)
        .map(|r| r.endpoint_or_cli.clone())
        .filter(|e| e.starts_with("http://") || e.starts_with("https://"))
}

fn ensure_active_provider_config(
    cfg: &mut NexumConfig,
    provider_id: &str,
    panel: &ModelPanel,
) -> Result<(), String> {
    if provider_id.is_empty() {
        return Ok(());
    }

    // `api_key` NO alcanza como prueba de "ya configurado". Ese atajo dejaba
    // pasar providers con credencial y SIN base_url, y el adaptador sin
    // endpoint cae al puente: por eso `-p --model qwen3:1.7b` salía a
    // CLIProxyAPI y volvía "unknown provider for model". Es el mismo uso de
    // api_key.is_empty() como proxy de estado que ya costó un microfix.
    let already_ok = cfg.config.providers.iter().any(|p| {
        p.id == provider_id && !p.api_key.is_empty() && !p.base_url.is_empty()
    });
    if already_ok {
        return Ok(());
    }

    // El endpoint lo sabe el route registry, que es su fuente. Si el provider
    // ya tiene credencial y sólo le falta la URL, se completa desde ahí sin
    // volver a resolver credenciales — barato y sin tocar el keystore.
    if let Some(endpoint) = endpoint_del_registry(provider_id) {
        if let Some(p) = cfg
            .config
            .providers
            .iter_mut()
            .find(|p| p.id == provider_id && !p.api_key.is_empty() && p.base_url.is_empty())
        {
            p.base_url = endpoint.clone();
            tracing::info!(provider_id, %endpoint, "base_url completado desde el route registry");
            return Ok(());
        }
    }
    let mut resolved = resolve_provider_credentials(provider_id)?;
    // Preferir el nombre de familia del catálogo ("OpenCode Zen") si el panel
    // lo conoce — es lo que el usuario vio como encabezado de sección.
    if let Some(family) = panel
        .model_choices
        .iter()
        .find(|c| c.provider_id == provider_id)
        .map(|c| c.family.clone())
    {
        resolved.display_name = family;
    }
    upsert_resolved_provider(cfg, &resolved);
    // Persistencia real entre reinicios: el workspace settings pisa los
    // providers globales al cargar, así que también hay que patchearlo.
    patch_workspace_provider(&resolved);
    Ok(())
}

fn provider_activation_error(provider_id: &str, model_id: &str, detail: &str) -> String {
    format!(
        "PROVIDER_ACTIVATION_FAILED: provider_id={provider_id} model_id={model_id}: {detail}"
    )
}

/// Invoca `provider_resolve.py <provider_id>` y parsea el JSON de stdout.
/// La key viaja solo por el pipe (nunca argv/env/logs). Solo lecturas locales
/// del lado Python — típicamente <100ms.
fn resolve_provider_credentials(provider_id: &str) -> Result<ResolvedProvider, String> {
    use std::process::{Command, Stdio};

    let script = nexum_acp::provider::routes::installed_provider_resolver_path()
        .map_err(|error| error.to_string())?;

    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(&script)
        .arg(provider_id)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("No se pudo lanzar python3: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let parsed: serde_json::Value = serde_json::from_str(line)
        .map_err(|_| "Respuesta inválida del resolver de credenciales.".to_string())?;
    if !parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Err(parsed
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("No se pudieron resolver credenciales.")
            .to_string());
    }
    let get = |k: &str| {
        parsed
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let r = ResolvedProvider {
        provider_id: get("provider_id"),
        display_name: get("display_name"),
        base_url: get("base_url"),
        api_key: get("api_key"),
        protocol: get("protocol"),
    };
    if r.api_key.is_empty() || r.base_url.is_empty() {
        return Err("Resolver devolvió credenciales incompletas.".to_string());
    }
    Ok(r)
}

#[cfg(test)]
#[path = "model_panel_test.rs"]
mod tests;
