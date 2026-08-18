use anyhow::Result;
use base64::Engine as _;
use ratatui::layout::Rect;

use crate::app::App;

/// Checks whether a mouse event falls within a given rectangle area.
pub fn mouse_in_rect(mouse: &ratatui::crossterm::event::MouseEvent, area: Rect) -> bool {
    mouse.row >= area.y
        && mouse.row < area.y + area.height
        && mouse.column >= area.x
        && mouse.column < area.x + area.width
}

/// Converts a terminal display column position to a character index within a line.
///
/// CJK and other full-width characters occupy 2 display columns. `mouse.column` is
/// a terminal column coordinate, but `CursorMove::Jump(row, col)` expects `col` as
/// a character index. This function accumulates `unicode_width` per character and
/// returns the largest character index whose display end does not exceed `display_col`.
pub fn display_col_to_char_idx(line: &str, display_col: usize) -> usize {
    let mut col = 0usize;
    for (char_idx, ch) in line.chars().enumerate() {
        if col >= display_col {
            return char_idx;
        }
        col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    // Click past end of line → return index at end of line
    line.chars().count()
}

/// Converts a mouse coordinate within a textarea's rendered area to a
/// textarea (row, char_idx) cursor position.
///
/// Four offsets are accounted for:
/// 1. **Block border + padding**: textarea renders within `Block::inner(area)`;
///    mouse coordinates must subtract these offsets to obtain text-area coordinates.
/// 2. **Vertical scroll offset**: when the text has more lines than the visible area
///    the textarea scrolls vertically (`top_row`); visible row 0 maps to text row `top_row`.
/// 3. **Horizontal scroll offset**: when text overflows horizontally the textarea
///    scrolls horizontally (`top_col`); visible column 0 maps to text column `top_col`
///    (in display columns).
/// 4. **CJK character width**: `Jump(row, col)` expects `col` as a character index,
///    not a display column. Conversion uses `unicode_width` per character.
///
/// `top_row` and `top_col` are inferred from cursor position because
/// `tui_textarea`'s viewport is private.
pub fn textarea_mouse_to_cursor(
    textarea: &tui_textarea::TextArea<'_>,
    textarea_area: ratatui::layout::Rect,
    mouse: &ratatui::crossterm::event::MouseEvent,
) -> (usize, usize) {
    // 1. Compute inner area (stripping border + padding)
    let inner = textarea
        .block()
        .map(|b| b.inner(textarea_area))
        .unwrap_or(textarea_area);
    let inner_width = inner.width as usize;
    let inner_height = inner.height as usize;

    // Mouse coordinates relative to inner area (saturating to avoid u16 overflow
    // when clicking on borders)
    let visual_row = mouse.row.saturating_sub(inner.y) as usize;
    let visual_col = mouse.column.saturating_sub(inner.x) as usize;

    // 2. Infer vertical scroll offset (top_row)
    // tui_textarea uses next_scroll_top logic: cursor < top_row => top_row = cursor;
    // cursor >= top_row + height => top_row = cursor + 1 - height; else unchanged.
    // Since viewport is private, we infer from cursor position:
    // cursor is always within [top_row, top_row + height), so top_row <= cursor_row
    let (cursor_row, cursor_col) = textarea.cursor();
    let scroll_row = cursor_row.saturating_sub(inner_height.saturating_sub(1));

    // 3. Infer horizontal scroll offset (top_col, in display columns)
    let cursor_line = textarea
        .lines()
        .get(cursor_row)
        .map(|s| s.as_str())
        .unwrap_or("");
    let cursor_display_col: usize = cursor_line
        .chars()
        .take(cursor_col)
        .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
        .sum();
    let scroll_col = cursor_display_col.saturating_sub(inner_width.saturating_sub(1));

    // 4. Text row and display column
    let target_row = scroll_row + visual_row;
    let text_display_col = visual_col + scroll_col;

    // 5. Convert display column to character index
    let target_row = target_row.min(textarea.lines().len().saturating_sub(1));
    let target_line = textarea
        .lines()
        .get(target_row)
        .map(|s| s.as_str())
        .unwrap_or("");
    let char_idx = display_col_to_char_idx(target_line, text_display_col);

    (target_row, char_idx)
}

/// Encodes RGBA pixel data as PNG, returning a base64 string and the PNG byte count.
pub fn rgba_to_png_base64(width: u32, height: u32, rgba_bytes: &[u8]) -> Result<(String, usize)> {
    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba_bytes)?;
    }
    let size = png_bytes.len();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Ok((b64, size))
}

/// Copies the current text selection to the system clipboard and updates UI
/// hints. Returns `true` if text was successfully copied.
pub fn copy_selection_to_clipboard(app: &mut App) -> bool {
    if let Some(text) = app
        .session_mgr
        .current_mut()
        .ui
        .text_selection
        .selected_text
        .take()
    {
        let char_count = text.chars().count();
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(&text);
        }
        app.session_mgr.current_mut().ui.copy_char_count = char_count;
        app.session_mgr.current_mut().ui.copy_message_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(2000));
        app.session_mgr.current_mut().ui.text_selection.clear();
        return true;
    }
    false
}

/// Copies the current panel selection to the system clipboard. Returns `true`
/// if text was successfully copied.
pub fn copy_panel_selection_to_clipboard(app: &mut App) -> bool {
    if let Some(text) = app
        .session_mgr
        .current_mut()
        .ui
        .panel_selection
        .selected_text
        .take()
    {
        let char_count = text.chars().count();
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(&text);
        }
        app.session_mgr.current_mut().ui.copy_char_count = char_count;
        app.session_mgr.current_mut().ui.copy_message_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(2000));
        app.session_mgr.current_mut().ui.panel_selection.clear();
        return true;
    }
    false
}

/// Copies the raw text of the last Nexum (AssistantBubble) response to the
/// system clipboard. Skips tool calls, reasoning, and user messages.
/// Returns `true` if text was successfully copied.
pub fn copy_last_response_to_clipboard(app: &mut App) -> bool {
    // Find the last AssistantBubble and delegate.
    let last_idx = app
        .session_mgr
        .current()
        .messages
        .view_messages
        .iter()
        .rposition(|vm| vm.assistant_raw_text().is_some());
    match last_idx {
        Some(idx) => copy_response_at_to_clipboard(app, idx),
        None => false,
    }
}

/// Copies the visible (already-redacted) text of the AssistantBubble at
/// `view_messages[idx]` to the system clipboard. Used by the clickable
/// "📋 Copiar" button (each button knows its own message index,
/// so clicking an older turn's button copies THAT turn's response) and by
/// `copy_last_response_to_clipboard` (Ctrl+C).
///
/// Only Text blocks are copied (`assistant_raw_text`): reasoning, tool
/// output and internal logs are never included. The blocks' text was
/// already passed through `redact_secrets` when the VM was built, so the
/// clipboard receives the same safe version the user sees on screen.
/// Returns `true` if text was successfully copied.
pub fn copy_response_at_to_clipboard(app: &mut App, idx: usize) -> bool {
    let text = {
        let messages = &app.session_mgr.current().messages.view_messages;
        match messages.get(idx).and_then(|vm| vm.assistant_raw_text()) {
            Some(t) => t,
            // idx viejo (la vista cambió entre el render del hitbox y el
            // click) o VM sin texto → no copiar nada equivocado.
            None => return false,
        }
    };

    let char_count = text.chars().count();
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(&text);
    }
    app.session_mgr.current_mut().ui.copy_char_count = char_count;
    app.session_mgr.current_mut().ui.copy_message_until =
        Some(std::time::Instant::now() + std::time::Duration::from_millis(2000));

    // Mark the AssistantBubble's footer as "copied" for ~2 seconds.
    let until = std::time::Instant::now() + std::time::Duration::from_millis(2000);
    if let Some(vm) = app
        .session_mgr
        .current_mut()
        .messages
        .view_messages
        .get_mut(idx)
    {
        if let crate::ui::message_view::MessageViewModel::AssistantBubble {
            copied_label_until,
            ..
        } = vm
        {
            *copied_label_until = Some(until);
        }
    }

    app.render_rebuild();
    true
}

/// Hit-test puro del click contra los hitboxes de botones "📋 Copiar"
/// registrados en el último frame. Devuelve el message_idx del botón
/// clickeado, si lo hay.
pub fn find_copy_button_hit(
    hitboxes: &[crate::app::CopyButtonHitbox],
    mouse: &ratatui::crossterm::event::MouseEvent,
) -> Option<usize> {
    hitboxes
        .iter()
        .find(|h| mouse_in_rect(mouse, h.rect))
        .map(|h| h.message_idx)
}

/// Determina si un MouseEventKind::Up corresponde a un click aislado (Down y
/// Up a distancia Manhattan ≤ 1) o al final de un drag. Usado para evitar que
/// soltar el mouse sobre el botón de copy dispare una copia accidental durante
/// una selección por arrastre.
pub fn is_isolated_click(
    down_pos: Option<(u16, u16)>,
    mouse: &ratatui::crossterm::event::MouseEvent,
) -> bool {
    down_pos.is_some_and(|(down_col, down_row)| {
        let dx = mouse.column.abs_diff(down_col);
        let dy = mouse.row.abs_diff(down_row);
        dx + dy <= 1
    })
}

#[cfg(test)]
#[path = "mouse_test.rs"]
mod mouse_test;
