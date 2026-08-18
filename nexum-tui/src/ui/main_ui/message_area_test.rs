use super::*;
use crate::ui::theme;

/// 检查 span 是否有选区背景色
fn has_selection_bg(style: Style) -> bool {
    matches!(style.bg, Some(theme::SELECTION_BG))
}

#[test]
fn test_highlight_line_spans_full_span() {
    let spans = vec![Span::from("Hello"), Span::from("World")];
    let result = highlight_line_spans(spans, 0, 10);
    assert_eq!(result.len(), 2);
    assert!(has_selection_bg(result[0].style));
    assert!(has_selection_bg(result[1].style));
}

#[test]
fn test_highlight_line_spans_partial_start() {
    let spans = vec![Span::from("Hello")];
    let result = highlight_line_spans(spans, 3, 10);
    // 前 3 字符原样，后 2 字符选区背景
    assert_eq!(result.len(), 2);
    assert!(!has_selection_bg(result[0].style));
    assert!(has_selection_bg(result[1].style));
    assert_eq!(result[0].content, "Hel");
    assert_eq!(result[1].content, "lo");
}

#[test]
fn test_highlight_line_spans_partial_both() {
    let spans = vec![Span::from("Hello")];
    let result = highlight_line_spans(spans, 1, 4);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].content, "H");
    assert!(!has_selection_bg(result[0].style));
    assert_eq!(result[1].content, "ell");
    assert!(has_selection_bg(result[1].style));
    assert_eq!(result[2].content, "o");
    assert!(!has_selection_bg(result[2].style));
}

#[test]
fn test_highlight_line_spans_multi_span() {
    let spans = vec![Span::from("Hel"), Span::from("lo Wo"), Span::from("rld")];
    let result = highlight_line_spans(spans, 2, 8);
    // 选中范围 char 2..8 = "llo Wo"
    // span0 "Hel": 前 2 原样 + 后 1 选区背景
    // span1 "lo Wo": 全部选区背景
    // span2 "rld": 不在选区（span2 starts at char 8）
    assert_eq!(result.len(), 4);
    assert_eq!(result[0].content, "He");
    assert!(!has_selection_bg(result[0].style));
    assert_eq!(result[1].content, "l");
    assert!(has_selection_bg(result[1].style));
    assert_eq!(result[2].content, "lo Wo");
    assert!(has_selection_bg(result[2].style));
    assert_eq!(result[3].content, "rld");
    assert!(!has_selection_bg(result[3].style));
}

#[test]
fn test_highlight_line_spans_outside() {
    let spans = vec![Span::from("Hello")];
    let result = highlight_line_spans(spans, 10, 15);
    assert_eq!(result.len(), 1);
    assert!(!has_selection_bg(result[0].style));
    assert_eq!(result[0].content, "Hello");
}

// ── Hitboxes del botón "📋 Copiar" (sprint copy-button 2026-07-08) ───────

fn make_wrap_map(rows_per_line: &[(u16, u16)]) -> Vec<WrappedLineInfo> {
    rows_per_line
        .iter()
        .enumerate()
        .map(|(idx, &(start, end))| WrappedLineInfo {
            line_idx: idx,
            visual_row_start: start,
            visual_row_end: end,
            plain_text: String::new(),
            char_widths: Vec::new(),
        })
        .collect()
}

fn area(x: u16, y: u16, w: u16, h: u16) -> Rect {
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

#[test]
fn test_hitbox_visible_sin_scroll() {
    // Línea del botón en fila visual 2, sin scroll → y = area.y + 2.
    let wrap_map = make_wrap_map(&[(0, 1), (1, 2), (2, 3)]);
    let buttons = vec![CachedCopyButton {
        line_idx: 2,
        message_idx: 1,
        col_start: 2,
        width: 20,
    }];
    let hb = compute_copy_button_hitboxes(&buttons, &wrap_map, 0, 10, area(5, 3, 80, 10));
    assert_eq!(hb.len(), 1);
    assert_eq!(hb[0].rect.x, 7, "x = area.x + col_start");
    assert_eq!(hb[0].rect.y, 5, "y = area.y + fila_visual - offset(0)");
    assert_eq!(hb[0].rect.width, 20);
    assert_eq!(hb[0].rect.height, 1);
    assert_eq!(hb[0].message_idx, 1);
}

#[test]
fn test_hitbox_respeta_scroll_offset() {
    // Con offset=4, la fila visual 6 queda en pantalla en y = area.y + 2.
    let wrap_map = make_wrap_map(&[(0, 3), (3, 6), (6, 7)]);
    let buttons = vec![CachedCopyButton {
        line_idx: 2,
        message_idx: 0,
        col_start: 2,
        width: 10,
    }];
    let hb = compute_copy_button_hitboxes(&buttons, &wrap_map, 4, 10, area(0, 0, 80, 10));
    assert_eq!(hb.len(), 1);
    assert_eq!(
        hb[0].rect.y, 2,
        "el scroll offset desplaza la y en pantalla"
    );
}

#[test]
fn test_hitbox_no_se_registra_scrolleado_arriba() {
    // Botón en fila visual 1, offset=5 → quedó arriba del viewport.
    let wrap_map = make_wrap_map(&[(0, 1), (1, 2)]);
    let buttons = vec![CachedCopyButton {
        line_idx: 1,
        message_idx: 0,
        col_start: 2,
        width: 10,
    }];
    let hb = compute_copy_button_hitboxes(&buttons, &wrap_map, 5, 10, area(0, 0, 80, 10));
    assert!(
        hb.is_empty(),
        "botón fuera del viewport (arriba) no es clickeable"
    );
}

#[test]
fn test_hitbox_no_se_registra_scrolleado_abajo() {
    // Botón en fila visual 30, viewport [0, 10) → abajo, fuera de pantalla.
    let wrap_map = make_wrap_map(&[(30, 31)]);
    let buttons = vec![CachedCopyButton {
        line_idx: 0,
        message_idx: 0,
        col_start: 2,
        width: 10,
    }];
    let hb = compute_copy_button_hitboxes(&buttons, &wrap_map, 0, 10, area(0, 0, 80, 10));
    assert!(
        hb.is_empty(),
        "botón fuera del viewport (abajo) no es clickeable"
    );
}

#[test]
fn test_hitbox_clampea_ancho_al_borde_derecho() {
    // Terminal angosto: col_start=2, width=20, area de 12 cols → clamp a 10.
    let wrap_map = make_wrap_map(&[(0, 1)]);
    let buttons = vec![CachedCopyButton {
        line_idx: 0,
        message_idx: 0,
        col_start: 2,
        width: 20,
    }];
    let hb = compute_copy_button_hitboxes(&buttons, &wrap_map, 0, 5, area(0, 0, 12, 5));
    assert_eq!(hb.len(), 1);
    assert_eq!(
        hb[0].rect.width, 10,
        "el hitbox no se sale del área de texto"
    );
}

#[test]
fn test_hitbox_line_idx_fuera_de_wrap_map_se_ignora() {
    // Cache desincronizado (carrera rebuild/render): no panic, no hitbox.
    let wrap_map = make_wrap_map(&[(0, 1)]);
    let buttons = vec![CachedCopyButton {
        line_idx: 99,
        message_idx: 0,
        col_start: 2,
        width: 10,
    }];
    let hb = compute_copy_button_hitboxes(&buttons, &wrap_map, 0, 5, area(0, 0, 80, 5));
    assert!(hb.is_empty(), "line_idx inválido se descarta sin panic");
}
