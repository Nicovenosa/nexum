use ratatui::style::Color;

use super::Theme;

/// Nexum 默认深色主题
///
/// 色值与 nexum-tui/src/ui/theme.rs 的常量一一对应。
/// 业务特有常量（TOOL_NAME=SAGE, SUB_AGENT=SAGE, MODEL_INFO=#A0825F）
/// 保留在 TUI 层，不在此处定义。
#[derive(Debug, Clone)]
pub struct DarkTheme;

impl Theme for DarkTheme {
    fn accent(&self) -> Color {
        Color::Rgb(0, 212, 170)
    } // ACCENT #00D4AA (Nexum teal)
    fn success(&self) -> Color {
        Color::Rgb(52, 211, 153)
    } // SUCCESS #34D399
    fn warning(&self) -> Color {
        Color::Rgb(251, 191, 36)
    } // WARNING #FBBF24
    fn error(&self) -> Color {
        Color::Rgb(251, 113, 133)
    } // ERROR #FB7185
    fn thinking(&self) -> Color {
        Color::Rgb(167, 139, 250)
    } // THINKING #A78BFA
    fn text(&self) -> Color {
        Color::Rgb(241, 245, 249)
    } // TEXT #F1F5F9
    fn muted(&self) -> Color {
        Color::Rgb(148, 163, 184)
    } // MUTED #94A3B8
    fn dim(&self) -> Color {
        Color::Rgb(100, 116, 139)
    } // DIM #64748B
    fn border(&self) -> Color {
        Color::Rgb(51, 65, 85)
    } // BORDER #334155
    fn border_active(&self) -> Color {
        Color::Rgb(0, 212, 170)
    } // = accent #00D4AA
    fn popup_bg(&self) -> Color {
        Color::Rgb(15, 23, 42)
    } // POPUP_BG #0F172A
    fn cursor_bg(&self) -> Color {
        Color::Rgb(30, 41, 59)
    } // CURSOR_BG #1E293B
    fn loading(&self) -> Color {
        Color::Rgb(0, 212, 170)
    } // LOADING #00D4AA (uses accent)

    fn user_bg(&self) -> Color {
        Color::Rgb(30, 41, 59)
    } // USER_BG #1E293B (slate-800)

    fn bash_border(&self) -> Color {
        Color::Rgb(0, 212, 170)
    } // BASH_BORDER #00D4AA (uses accent)
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("presets_test.rs");
}
