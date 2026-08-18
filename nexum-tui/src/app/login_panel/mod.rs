use super::{panel_list::PanelList, App};
use crate::app::FieldTextarea;
use crate::config::{NexumConfig, ProviderConfig, ProviderModels};

// ─── 默认模型名常量表 ─────────────────────────────────────────────────────────

/// (provider_type, opus, sonnet, haiku)
const DEFAULT_MODELS: &[(&str, &str, &str, &str)] = &[
    (
        "anthropic",
        "claude-opus-4-7",
        "claude-sonnet-4-6",
        "claude-haiku-4-5",
    ),
    ("openai", "gpt-4o", "gpt-4o-mini", "gpt-3.5-turbo"),
];

/// provider_type 循环切换列表
const PROVIDER_TYPES: &[&str] = &["openai", "anthropic"];

// ─── 枚举 ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum LoginPanelMode {
    Browse,
    Edit,
    New,
    ConfirmDelete,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoginEditField {
    Name,
    Type,
    BaseUrl,
    ApiKey,
    OpusModel,
    SonnetModel,
    HaikuModel,
}

impl LoginEditField {
    pub fn next(&self) -> Self {
        match self {
            Self::Name => Self::Type,
            Self::Type => Self::BaseUrl,
            Self::BaseUrl => Self::ApiKey,
            Self::ApiKey => Self::OpusModel,
            Self::OpusModel => Self::SonnetModel,
            Self::SonnetModel => Self::HaikuModel,
            Self::HaikuModel => Self::Name,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::Name => Self::HaikuModel,
            Self::Type => Self::Name,
            Self::BaseUrl => Self::Type,
            Self::ApiKey => Self::BaseUrl,
            Self::OpusModel => Self::ApiKey,
            Self::SonnetModel => Self::OpusModel,
            Self::HaikuModel => Self::SonnetModel,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Name => "Name        ",
            Self::Type => "Type        ",
            Self::BaseUrl => "Base URL    ",
            Self::ApiKey => "API Key     ",
            Self::OpusModel => "Primary model ",
            Self::SonnetModel => "Balanced model",
            Self::HaikuModel => "Fast model    ",
        }
    }
}

// ─── LoginPanel ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct LoginPanel {
    /// provider 列表快照（从 NexumConfig 获取）
    pub providers: Vec<ProviderConfig>,
    /// 当前模式
    pub mode: LoginPanelMode,
    /// Browse 模式光标管理
    pub(crate) browse_list: PanelList<()>,
    /// 正在编辑的字段（Edit/New 模式下）
    pub edit_field: LoginEditField,
    /// 编辑缓冲区
    pub field_name: FieldTextarea,
    pub buf_type: String, // Type 不可编辑，保持 String
    pub field_base_url: FieldTextarea,
    pub field_api_key: FieldTextarea,
    pub field_opus_model: FieldTextarea,
    pub field_sonnet_model: FieldTextarea,
    pub field_haiku_model: FieldTextarea,
}

impl LoginPanel {
    /// 从 NexumConfig 初始化面板（Browse 模式，光标定位到 active_provider_id 对应的 Provider）
    pub fn from_config(cfg: &NexumConfig) -> Self {
        let providers = cfg.config.providers.clone();
        let cursor = providers
            .iter()
            .position(|p| p.id == cfg.config.active_provider_id)
            .unwrap_or(0);
        let mut browse_list = PanelList::new();
        browse_list.set_items(vec![(); providers.len()]);
        browse_list.move_cursor_to(cursor);
        Self {
            providers,
            mode: LoginPanelMode::Browse,
            browse_list,
            edit_field: LoginEditField::Name,
            field_name: FieldTextarea::single_line(),
            buf_type: String::new(),
            field_base_url: FieldTextarea::single_line(),
            field_api_key: FieldTextarea::single_line(),
            field_opus_model: FieldTextarea::single_line(),
            field_sonnet_model: FieldTextarea::single_line(),
            field_haiku_model: FieldTextarea::single_line(),
        }
    }

    // ── Browse 模式操作 ──────────────────────────────────────────────────────

    /// 当前光标位置（Browse 模式下标记选中行）
    pub fn cursor(&self) -> usize {
        self.browse_list.cursor()
    }

    /// 列表上下移动光标（clamp 模式，不循环）
    pub fn move_cursor(&mut self, delta: isize) {
        self.browse_list.move_cursor(delta);
    }

    /// 进入编辑模式（编辑光标处的 provider）
    pub fn enter_edit(&mut self) {
        if let Some(p) = self.providers.get(self.cursor()) {
            self.field_name.set_value(p.display_name());
            self.buf_type = p.provider_type.clone();
            self.field_base_url.set_value(&p.base_url);
            self.field_api_key.set_value(&p.api_key);
            self.field_opus_model.set_value(&p.models.opus);
            self.field_sonnet_model.set_value(&p.models.sonnet);
            self.field_haiku_model.set_value(&p.models.haiku);
            self.edit_field = LoginEditField::Name;
            self.mode = LoginPanelMode::Edit;
        }
    }

    /// 进入新建模式（清空所有缓冲，type 默认 "openai"，模型名按 type 自动填充）
    pub fn enter_new(&mut self) {
        self.field_name.clear();
        self.buf_type = "openai".to_string();
        self.field_base_url.clear();
        self.field_api_key.clear();
        self.field_opus_model.clear();
        self.field_sonnet_model.clear();
        self.field_haiku_model.clear();
        self.auto_fill_models_for_type();
        self.edit_field = LoginEditField::Name;
        self.mode = LoginPanelMode::New;
    }

    /// 进入删除确认模式
    pub fn request_delete(&mut self) {
        if !self.providers.is_empty() {
            self.mode = LoginPanelMode::ConfirmDelete;
        }
    }

    /// 选中（激活）光标处的 Provider，写入 cfg
    pub fn select_provider(&mut self, cfg: &mut NexumConfig) {
        if let Some(p) = self.providers.get(self.cursor()) {
            cfg.config.active_provider_id = p.id.clone();
        }
    }

    /// 取消删除确认，回到浏览模式
    pub fn cancel_delete(&mut self) {
        self.mode = LoginPanelMode::Browse;
    }

    // ── Edit/New 模式操作 ────────────────────────────────────────────────────

    /// 字段导航：下一个字段
    pub fn field_next(&mut self) {
        self.edit_field = self.edit_field.next();
    }

    /// 字段导航：上一个字段
    pub fn field_prev(&mut self) {
        self.edit_field = self.edit_field.prev();
    }

    /// 循环切换 provider_type（Space 键，仅在 edit_field == Type 时生效）
    /// 切换后自动调用 auto_fill_models_for_type 更新模型名默认值
    pub fn cycle_type(&mut self) {
        if self.edit_field == LoginEditField::Type {
            let cur = PROVIDER_TYPES
                .iter()
                .position(|&t| t == self.buf_type)
                .unwrap_or(0);
            self.buf_type = PROVIDER_TYPES[(cur + 1) % PROVIDER_TYPES.len()].to_string();
            self.auto_fill_models_for_type();
        }
    }

    /// 获取当前编辑字段的 FieldTextarea 可变引用
    pub fn active_field(&mut self) -> Option<&mut FieldTextarea> {
        match self.edit_field {
            LoginEditField::Name => Some(&mut self.field_name),
            LoginEditField::Type => None,
            LoginEditField::BaseUrl => Some(&mut self.field_base_url),
            LoginEditField::ApiKey => Some(&mut self.field_api_key),
            LoginEditField::OpusModel => Some(&mut self.field_opus_model),
            LoginEditField::SonnetModel => Some(&mut self.field_sonnet_model),
            LoginEditField::HaikuModel => Some(&mut self.field_haiku_model),
        }
    }

    /// 粘贴文本到当前活动字段（过滤换行符，Type 字段忽略粘贴）
    pub fn paste_text(&mut self, text: &str) {
        let text: String = text.chars().filter(|&c| c != '\n' && c != '\r').collect();
        if let Some(field) = self.active_field() {
            field.insert_text(&text);
        }
    }

    // ── Type 切换自动填充 ────────────────────────────────────────────────────

    /// Type 切换时自动填充模型名默认值
    /// 规则：检测三个模型名字段是否为空或等于旧 provider_type 的默认值；若是则填入新 type 的默认值
    pub fn auto_fill_models_for_type(&mut self) {
        let new_defaults = DEFAULT_MODELS
            .iter()
            .find(|(t, _, _, _)| *t == self.buf_type);
        let (opus_default, sonnet_default, haiku_default) = match new_defaults {
            Some((_, o, s, h)) => (o.to_string(), s.to_string(), h.to_string()),
            None => return, // 未知 provider_type，不自动填充
        };

        // 收集所有 provider_type 的默认值作为"旧默认值"候选
        let all_defaults: Vec<(String, String, String)> = DEFAULT_MODELS
            .iter()
            .map(|(_, o, s, h)| (o.to_string(), s.to_string(), h.to_string()))
            .collect();

        let is_default_or_empty = |val: &str| -> bool {
            if val.is_empty() {
                return true;
            }
            all_defaults
                .iter()
                .any(|(o, s, h)| val == o || val == s || val == h)
        };

        if is_default_or_empty(&self.field_opus_model.value()) {
            self.field_opus_model.set_value(&opus_default);
        }
        if is_default_or_empty(&self.field_sonnet_model.value()) {
            self.field_sonnet_model.set_value(&sonnet_default);
        }
        if is_default_or_empty(&self.field_haiku_model.value()) {
            self.field_haiku_model.set_value(&haiku_default);
        }
    }

    // ── 保存/删除操作 ──────────────────────────────────────────────────────────

    /// 将编辑/新建的内容保存到 NexumConfig，并更新内部 providers 快照
    /// 返回 true 表示成功
    /// 新建 Provider 后，active_provider_id 为空时自动设置为新建的 Provider ID
    pub fn apply_edit(&mut self, cfg: &mut NexumConfig) -> bool {
        let is_new = self.mode == LoginPanelMode::New;
        let name = self.field_name.value();
        let id = if is_new {
            if name.trim().is_empty() {
                return false;
            }
            name.trim().to_lowercase().replace(' ', "_")
        } else {
            self.providers
                .get(self.cursor())
                .map(|p| p.id.clone())
                .unwrap_or_default()
        };

        if id.is_empty() {
            return false;
        }

        let mut p = ProviderConfig {
            id: id.clone(),
            provider_type: self.buf_type.clone(),
            api_key: self.field_api_key.value(),
            base_url: self.field_base_url.value(),
            name: if name.trim().is_empty() {
                None
            } else {
                Some(name.trim().to_string())
            },
            models: ProviderModels {
                opus: self.field_opus_model.value(),
                sonnet: self.field_sonnet_model.value(),
                haiku: self.field_haiku_model.value(),
            },
            thinking: None,
            extra: Default::default(),
        };

        // 编辑模式：保留原有的 extra 字段
        if self.mode == LoginPanelMode::Edit {
            if let Some(orig) = self.providers.get(self.cursor()) {
                p.extra = orig.extra.clone();
            }
        }

        if is_new {
            cfg.config.providers.push(p);
            // active_provider_id 为空时自动设置
            if cfg.config.active_provider_id.is_empty() {
                cfg.config.active_provider_id = id;
            }
        } else if let Some(existing) = cfg.config.providers.iter_mut().find(|x| x.id == id) {
            *existing = p;
        }

        self.providers = cfg.config.providers.clone();
        self.browse_list.set_items(vec![(); self.providers.len()]);
        self.browse_list.clamp_cursor();
        self.mode = LoginPanelMode::Browse;
        true
    }

    /// 确认删除光标处的 provider，写入 cfg
    pub fn confirm_delete(&mut self, cfg: &mut NexumConfig) {
        if let Some(p) = self.providers.get(self.cursor()) {
            let id = p.id.clone();
            cfg.config.providers.retain(|x| x.id != id);
            self.providers = cfg.config.providers.clone();
            self.browse_list.set_items(vec![(); self.providers.len()]);
            self.browse_list.clamp_cursor();
            // 如果删除的是当前激活的 provider，清空 active_provider_id
            if cfg.config.active_provider_id == id {
                cfg.config.active_provider_id.clear();
            }
        }
        self.mode = LoginPanelMode::Browse;
    }
}

mod component;

#[cfg(test)]
#[path = "login_panel_test.rs"]
mod tests;
