/// TUI 统一颜色主题（Nexum Dark 配色方案）
///
/// 设计哲学：Slate 灰层级 + Nexum 青绿色品牌色。
/// 背景透明——不使用任何 bg() 颜色（弹窗光标行和用户消息区除外）。
/// 信息层级用亮度区分（TEXT/MUTED/DIM），颜色表达状态语义。
use ratatui::style::Color;

// ── 强调色（单一主色）────────────────────────────────────────────────────────

/// Nexum 青绿 — 唯一主交互色，品牌色 #00D4AA
pub const ACCENT: Color = Color::Rgb(0, 212, 170);

/// Nexum 霓虹绿 — logo 和 mascota 专用 #39FF14 (比 ACCENT 更亮更饱和)
pub const NEXUM_GREEN: Color = Color::Rgb(57, 255, 20);

// ── 功能色 ───────────────────────────────────────────────────────────────────

/// 柔和绿 — 成功/工具名/在线状态 #34D399
pub const SAGE: Color = Color::Rgb(52, 211, 153);

/// 柔和琥珀 — 次要强调/警告 #FBBF24
pub const WARNING: Color = Color::Rgb(251, 191, 36);

/// 柔和玫瑰红 — 错误/拒绝 #FB7185
pub const ERROR: Color = Color::Rgb(251, 113, 133);

/// 柔和紫 — 推理/CoT 思考内容 #A78BFA
pub const THINKING: Color = Color::Rgb(167, 139, 250);

// ── 文字层级（三级亮度）──────────────────────────────────────────────────────

/// Slate-100 — 主文字 #F1F5F9
pub const TEXT: Color = Color::Rgb(241, 245, 249);

/// Slate-400 — 标签/路径/辅助信息 #94A3B8
pub const MUTED: Color = Color::Rgb(148, 163, 184);

/// Slate-500 — 占位/已完成项/分隔符 #64748B
pub const DIM: Color = Color::Rgb(100, 116, 139);

// ── 边框 ─────────────────────────────────────────────────────────────────────

/// Slate-700 — 空闲边框 #334155
pub const BORDER: Color = Color::Rgb(51, 65, 85);

/// Slate-600 — 非活跃 session 分隔线 #475569
pub const BORDER_DIM: Color = Color::Rgb(71, 85, 105);

/// 激活边框 — Nexum 青绿
pub const BORDER_ACTIVE: Color = ACCENT;

// ── 弹窗专用 ─────────────────────────────────────────────────────────────────

/// Slate-900 — 弹窗底色 #0F172A
pub const POPUP_BG: Color = NEXUM_BG;

/// Fondo Nexum #010401 — negro con tinte verde, paleta oficial del TUI.
/// (Antes los popups usaban Slate-900 #0F172A, el "fondo azul" que no
/// matcheaba la paleta — fix UX popups 2026-07-05.)
pub const NEXUM_BG: Color = Color::Rgb(1, 4, 1);

/// Verde primario Nexum #1CE822 — bordes de popups e ítems activos.
pub const NEXUM_PRIMARY: Color = Color::Rgb(28, 232, 34);

/// Slate-800 — 光标行背景（列表选中行）#1E293B
pub const CURSOR_BG: Color = Color::Rgb(30, 41, 59);

/// Accent — Loading/Spinner 专用 #00D4AA
pub const LOADING: Color = Color::Rgb(0, 212, 170);

/// Slate-800 — 用户消息背景色 #1E293B
pub const USER_BG: Color = Color::Rgb(30, 41, 59);

/// 文本选区背景色 #1E3A5F（深色主题下选中蓝的暗色版本）
pub const SELECTION_BG: Color = Color::Rgb(30, 58, 95);

/// 选中行前景色（列表高亮文字，青绿系）#5EEAD4
pub const SELECTED_FG: Color = Color::Rgb(94, 234, 212);

/// Nexum 青绿 — Bash 工具调用边框色 #00D4AA
pub const BASH_BORDER: Color = Color::Rgb(0, 212, 170);

/// SubAgent 嵌套消息背景色 #1E293B（比终端背景略亮，形成视觉容器）
pub const SUB_AGENT_BG: Color = Color::Rgb(30, 41, 59);

// ── 语义别名 ─────────────────────────────────────────────────────────────────

/// 工具名颜色（= SAGE）
pub const TOOL_NAME: Color = SAGE;

/// SubAgent 颜色（= SAGE）
pub const SUB_AGENT: Color = SAGE;

/// 模型信息颜色 — Slate 灰，对应 #94A3B8（状态栏模型名，不抢眼）
pub const MODEL_INFO: Color = Color::Rgb(148, 163, 184);

// ── 背景不透明色（UX FIX 04）────────────────────────────────────────────────

/// 主聊天区背景 #0B1110 — 深青黑，比终端默认黑更暖
pub const CHAT_BG: Color = Color::Rgb(11, 17, 16);

/// 输入区背景 #101817 — 略亮于 CHAT_BG
pub const INPUT_BG: Color = Color::Rgb(16, 24, 23);

/// 状态栏/头部背景 #080D0C — 最深，锚定底部
pub const BAR_BG: Color = Color::Rgb(8, 13, 12);

// ── 卡片边框色（UX FIX 05）────────────────────────────────────────────────

/// 用户消息卡片边框 #1F3B35 — 暗青绿
pub const USER_BORDER: Color = Color::Rgb(31, 59, 53);

/// Nexum 回复卡片边框 #00D4AA — accent
pub const NEXUM_BORDER: Color = Color::Rgb(0, 212, 170);

/// Nexum 回复卡片背景 #0B1514 — 比 CHAT_BG 略亮
pub const NEXUM_CARD_BG: Color = Color::Rgb(11, 21, 20);

/// 用户卡片背景 #101817 — INPUT_BG
pub const USER_CARD_BG: Color = Color::Rgb(16, 24, 23);

/// 错误卡片背景 #1A0F14 — 暗红
pub const ERROR_CARD_BG: Color = Color::Rgb(26, 15, 20);

/// 工具卡片背景 #141810 — 暗琥珀
pub const TOOL_CARD_BG: Color = Color::Rgb(20, 24, 16);
