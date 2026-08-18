//! Viewport scrolling compartido para los popups de comandos
//! (/modelo, /provedor). Fix UX popups Bug 1: las listas largas eran
//! inasequibles porque el render truncaba en la altura visible sin offset.

/// Ajusta el offset del viewport para que el rango del cursor
/// (`cursor_start..=cursor_end`, en líneas) quede visible.
///
/// - `offset`: primer línea visible actual.
/// - `viewport`: alto visible en líneas.
/// - `total`: cantidad total de líneas.
///
/// Devuelve el nuevo offset, clampeado a `[0, total - viewport]`.
pub(crate) fn ensure_visible(
    mut offset: usize,
    cursor_start: usize,
    cursor_end: usize,
    viewport: usize,
    total: usize,
) -> usize {
    if viewport == 0 || total <= viewport {
        return 0;
    }
    if cursor_start < offset {
        offset = cursor_start;
    }
    let cursor_end = cursor_end.min(total.saturating_sub(1));
    if cursor_end >= offset + viewport {
        offset = cursor_end + 1 - viewport;
    }
    offset.min(total - viewport)
}

/// Indicadores de contenido fuera de vista: (arriba, abajo).
pub(crate) fn overflow_indicators(offset: usize, viewport: usize, total: usize) -> (bool, bool) {
    (offset > 0, offset + viewport < total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lista_corta_no_scrollea() {
        assert_eq!(ensure_visible(5, 2, 2, 10, 8), 0);
    }

    #[test]
    fn cursor_debajo_del_viewport_empuja_offset() {
        // viewport 10, cursor en línea 25 → offset = 16 (25 visible al fondo).
        assert_eq!(ensure_visible(0, 25, 25, 10, 40), 16);
    }

    #[test]
    fn cursor_arriba_del_viewport_trae_offset() {
        assert_eq!(ensure_visible(20, 5, 5, 10, 40), 5);
    }

    #[test]
    fn offset_clampeado_al_final() {
        assert_eq!(ensure_visible(0, 39, 39, 10, 40), 30);
        assert_eq!(ensure_visible(35, 39, 39, 10, 40), 30);
    }

    #[test]
    fn rango_multilinea_visible_completo() {
        // Fila de provider con detalle expandido (líneas 12..=17), viewport 10.
        let off = ensure_visible(0, 12, 17, 10, 40);
        assert!(off <= 12 && 17 < off + 10, "off={off}");
    }

    #[test]
    fn indicadores_de_overflow() {
        assert_eq!(overflow_indicators(0, 10, 40), (false, true));
        assert_eq!(overflow_indicators(16, 10, 40), (true, true));
        assert_eq!(overflow_indicators(30, 10, 40), (true, false));
        assert_eq!(overflow_indicators(0, 10, 8), (false, false));
    }

    #[test]
    fn viewport_scroll_40_items_cursor_30_visible() {
        // Simula el caso del prompt: 40 ítems (una línea cada uno),
        // el cursor baja 30 veces; con viewport de 12 el ítem 30 tiene que
        // quedar dentro de la ventana.
        let viewport = 12;
        let total = 40;
        let mut offset = 0;
        let mut cursor = 0;
        for _ in 0..30 {
            cursor += 1;
            offset = ensure_visible(offset, cursor, cursor, viewport, total);
        }
        assert_eq!(cursor, 30);
        assert!(
            cursor >= offset && cursor < offset + viewport,
            "cursor {cursor} fuera del viewport [{offset}, {})",
            offset + viewport
        );
    }
}
