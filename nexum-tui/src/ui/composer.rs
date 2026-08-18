//! Composer (input line) invariants + feature flags.
//!
//! Sprint COMPOSER GHOST TEXT (2026-07-07): el composer sólo puede mostrar el
//! `input_buffer` real del usuario, su cursor y su selección. NUNCA puede
//! mostrar predicciones/ghost text generadas por el modelo, restos de frames
//! anteriores (stale cells), ni output de tools/status.
//!
//! El bug reportado: aparecía texto tipo `nexum-agent 的 ReAct ...` en la línea
//! de input sin que el usuario lo escribiera. Origen: la feature de
//! "prediction" heredada de Peri — tras cada respuesta, un LLM fork predice
//! qué escribiría el usuario y el resultado se pinta como overlay DIM sobre el
//! composer. En Nexum eso confunde (parece que el input está sucio) y a veces
//! el LLM alucina (docs de Peri en chino). Se desactiva por defecto.

use ratatui::layout::Rect;

use crate::ui::theme;

/// ¿Están habilitadas las predicciones de input (ghost text)?
///
/// Default: **OFF** en Nexum. Opt-in explícito con `NEXUM_PREDICTIONS=on|1|true`.
/// Cuando está off: no se dispara el LLM fork de predicción (ahorra la llamada)
/// y no se renderiza nada en el composer aunque llegue una notificación vieja.
///
/// `NEXUM_PUBLIC_DEMO=1` fuerza esto a off SIEMPRE, sin importar
/// `NEXUM_PREDICTIONS` (sprint public-demo 2026-07-07 — ver `demo_mode`).
pub fn predictions_enabled() -> bool {
    if crate::ui::demo_mode::public_demo_enabled() {
        return false;
    }
    matches!(
        std::env::var("NEXUM_PREDICTIONS")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "on" | "1" | "true" | "yes"
    )
}

/// ¿Está activo el debug de render del composer? (`NEXUM_RENDER_DEBUG=1`)
///
/// No loguea contenido sensible: sólo rects, longitudes y flags de overlays.
/// `NEXUM_PUBLIC_DEMO=1` lo fuerza a off (defensa en profundidad: nada de
/// logs de debug con rutas internas durante una grabación pública).
pub fn render_debug_enabled() -> bool {
    if crate::ui::demo_mode::public_demo_enabled() {
        return false;
    }
    matches!(
        std::env::var("NEXUM_RENDER_DEBUG")
            .unwrap_or_default()
            .as_str(),
        "1" | "on" | "true"
    )
}

/// Limpia COMPLETAMENTE el Rect del composer antes de dibujar: resetea símbolo,
/// fg, bg y modifier de todas las celdas. Sin esto, una línea larga previa (o
/// un overlay de prediction ya desactivado) deja restos: los widgets que se
/// dibujan encima sólo pintan las celdas que ocupan, no las que quedaron con
/// contenido de un frame anterior.
///
/// El textarea (con su Block/borde) se dibuja DESPUÉS encima de esta base
/// limpia, así que el borde y el contenido real quedan intactos.
pub fn clear_composer_area(buf: &mut ratatui::buffer::Buffer, area: Rect) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_char(' ');
                cell.bg = theme::INPUT_BG;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Las env vars son globales al proceso; los tests corren multi-thread.
    // Este mutex serializa los que tocan NEXUM_PREDICTIONS para evitar races.

    fn with_env<T>(key: &str, val: Option<&str>, f: impl FnOnce() -> T) -> T {
        let prev = std::env::var(key).ok();
        match val {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        let out = f();
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        out
    }

    #[test]
    fn test_predictions_flag_matrix() {
        let _g = crate::ui::demo_mode::test_env_lock();
        // Default (sin la var): off.
        assert!(
            !with_env("NEXUM_PREDICTIONS", None, predictions_enabled),
            "default debe ser off"
        );
        // Opt-in explícito.
        for v in ["on", "1", "true", "yes", "ON", "True"] {
            assert!(
                with_env("NEXUM_PREDICTIONS", Some(v), predictions_enabled),
                "{v} debería habilitar"
            );
        }
        // Valores que NO habilitan.
        for v in ["off", "0", "false", "no", ""] {
            assert!(
                !with_env("NEXUM_PREDICTIONS", Some(v), predictions_enabled),
                "{v} NO debería habilitar"
            );
        }
    }

    #[test]
    fn test_clear_composer_resets_stale_cells() {
        use ratatui::buffer::Buffer;
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        // Simula restos de un frame anterior: texto CJK largo + fg custom.
        for (i, ch) in "nexum-agent 的 ReAct".chars().enumerate() {
            if let Some(cell) = buf.cell_mut((i as u16, 0)) {
                cell.set_char(ch);
                cell.fg = theme::DIM;
            }
        }
        clear_composer_area(&mut buf, area);
        // Ninguna celda debe conservar un símbolo distinto de espacio.
        for y in 0..3 {
            for x in 0..20 {
                let cell = buf.cell((x, y)).unwrap();
                assert_eq!(cell.symbol(), " ", "celda ({x},{y}) quedó con resto");
            }
        }
    }
}
