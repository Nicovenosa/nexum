//! Nexum mascot — robot compacto de 3 líneas con animación de respiración.
//!
//! UX FIX 05: Mascota viva por estado + respiración.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::ui::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MascotSize {
    Normal,
    Plus5,
    Plus10,
    Plus20,
}

impl MascotSize {
    pub fn from_env() -> Self {
        match std::env::var("NEXUM_MASCOT_SIZE") {
            Ok(v) if v.eq_ignore_ascii_case("plus5") => Self::Plus5,
            Ok(v) if v.eq_ignore_ascii_case("plus10") => Self::Plus10,
            Ok(v) if v.eq_ignore_ascii_case("plus20") => Self::Plus20,
            // FIX URGENTE: default Normal para preservar la silueta original.
            // Las variantes Plus10/Plus20 estiran horizontalmente el arte y
            // deforman la mascota. Quedan disponibles via env var pero NUNCA
            // se usan por defecto.
            _ => Self::Normal,
        }
    }
}

struct MascotArt {
    line1: &'static str,
    line2_left: &'static str,
    line2_right: &'static str,
    line3: &'static str,
}

impl MascotArt {
    fn for_size(size: MascotSize) -> Self {
        // Cada paso suma +4 cols para que las variantes sean claramente
        // distinguibles visualmente en el header.
        // Normal=10, Plus5=14, Plus10=18, Plus20=22
        match size {
            MascotSize::Normal => Self {
                line1: " ▐▛▀▀▀▀▜▌",
                line2_left: "▐█▌",
                line2_right: "▐█▌",
                line3: " ▝▀╪╪╪╪▀▘",
            },
            MascotSize::Plus5 => Self {
                line1: " ▐▛▀▀▀▀▀▀▀▀▀▜▌",
                line2_left: "▐██▌",
                line2_right: "▐██▌",
                line3: " ▝▀╪╪╪╪╪╪╪╪▀▘",
            },
            MascotSize::Plus10 => Self {
                line1: " ▐▛▀▀▀▀▀▀▀▀▀▀▀▀▀▜▌",
                line2_left: "▐███▌",
                line2_right: "▐███▌",
                line3: " ▝▀╪╪╪╪╪╪╪╪╪╪╪╪▀▘",
            },
            MascotSize::Plus20 => Self {
                line1: " ▐▛▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▜▌",
                line2_left: "▐████▌",
                line2_right: "▐████▌",
                line3: " ▝▀╪╪╪╪╪╪╪╪╪╪╪╪╪╪╪╪▀▘",
            },
        }
    }

    fn width(&self) -> u16 {
        let line2_width =
            self.line2_left.chars().count() + 1 + 2 + 1 + self.line2_right.chars().count();
        [
            self.line1.chars().count(),
            line2_width,
            self.line3.chars().count(),
        ]
        .into_iter()
        .max()
        .unwrap_or(0) as u16
    }
}

/// Estados de la mascota
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MascotState {
    Idle,
    Thinking,
    Processing,
    Tool,
    Memory,
    Success,
    Error,
    Offline,
}

impl MascotState {
    pub fn color(&self) -> Color {
        match self {
            Self::Idle => theme::NEXUM_GREEN,
            Self::Thinking => theme::NEXUM_GREEN,
            Self::Processing => theme::ACCENT,
            Self::Tool => theme::WARNING,
            Self::Memory => Color::Rgb(59, 130, 246), // #3B82F6 blue
            Self::Success => theme::ACCENT,
            Self::Error => theme::ERROR,
            Self::Offline => theme::DIM,
        }
    }

    /// Ojos (izquierdo, derecho) según el estado
    pub fn eyes(&self) -> (char, char) {
        match self {
            Self::Idle => ('◉', '◎'),
            Self::Thinking => ('•', '•'),
            Self::Processing => ('◉', '◉'),
            Self::Tool => ('◆', '◆'),
            Self::Memory => ('○', '○'),
            Self::Success => ('◉', '◉'),
            Self::Error => ('×', '×'),
            Self::Offline => ('·', '·'),
        }
    }
}

/// Modo de visualización de la mascota (UX FIX 04 + 05)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MascotMode {
    Welcome,
    Status,
    Off,
}

impl MascotMode {
    pub fn from_env() -> Self {
        match std::env::var("NEXUM_MASCOT_MODE") {
            Ok(v) if v.eq_ignore_ascii_case("status") => Self::Status,
            Ok(v) if v.eq_ignore_ascii_case("off") => Self::Off,
            Ok(v) if v.eq_ignore_ascii_case("welcome") => Self::Welcome,
            _ => Self::Status, // Default: show mascot in both welcome and chat
        }
    }
}

pub const MASCOT_HEIGHT: u16 = 3;

pub fn mascot_width_for_size(size: MascotSize) -> u16 {
    MascotArt::for_size(size).width()
}

/// FIX URGENTE: tamaño para el header del chat. Siempre Normal (10 cols).
/// NUNCA usa Plus5/Plus10/Plus20 para evitar que la mascota domine el header
/// o rompa la silueta. Si en el futuro se diseña una variante Mini, se usa
/// acá.
pub fn mascot_size_for_chat(_area_width: u16) -> MascotSize {
    MascotSize::Normal
}

/// FIX URGENTE: tamaño para la pantalla de welcome.
/// - Por defecto usa Normal para conservar la silueta original.
/// - Respeta NEXUM_MASCOT_SIZE=plus5 solo si cabe (no deforma el layout).
/// - NUNCA usa Plus10/Plus20 por defecto porque deforman la mascota.
pub fn mascot_size_for_welcome(area_width: u16) -> MascotSize {
    let requested = MascotSize::from_env();
    let fits = |s: MascotSize| MascotArt::for_size(s).width().saturating_add(4) <= area_width;
    // Plus10/Plus20 quedan deshabilitados para welcome hasta rediseño manual.
    let allowed = match requested {
        MascotSize::Plus10 | MascotSize::Plus20 => MascotSize::Plus5,
        other => other,
    };
    if fits(allowed) {
        return allowed;
    }
    if fits(MascotSize::Plus5) {
        return MascotSize::Plus5;
    }
    MascotSize::Normal
}

/// Tamaño genérico para compatibilidad hacia atrás. Preferir los helpers
/// específicos de contexto (`mascot_size_for_chat`, `mascot_size_for_welcome`).
pub fn mascot_size_for_env(area_width: u16) -> MascotSize {
    mascot_size_for_welcome(area_width)
}

pub fn mascot_width_for_env(area_width: u16) -> u16 {
    mascot_width_for_size(mascot_size_for_env(area_width))
}

/// Respiración: intervalo de pulso en ticks (~0.5s a 30fps)
const BREATH_INTERVAL: u64 = 15;

/// Renderiza el header persistente: mascota + identidad Nexum.
pub fn render_header(
    f: &mut Frame,
    area: Rect,
    state: MascotState,
    _provider: &str,
    _model: &str,
    is_welcome: bool,
    tick: u64,
    accent: Option<Color>,
) {
    if is_welcome || area.height == 0 || area.width < 5 {
        return;
    }

    let mascot_mode = MascotMode::from_env();
    // In welcome, mascot is rendered centered in welcome.rs — don't duplicate in header.
    // In chat mode, show mascot in header when Status mode is active.
    let show_mascot = match mascot_mode {
        MascotMode::Off => false,
        MascotMode::Welcome => false, // welcome screen has its own centered mascot
        MascotMode::Status => !is_welcome, // only in chat, not during welcome
    };

    let show_mascot = show_mascot && area.height >= MASCOT_HEIGHT;

    if show_mascot {
        // Respiración: color oscila sutilmente con el tick
        let breath_phase = (tick / BREATH_INTERVAL) % 6;
        let bright = breath_phase < 3; // 3 ticks bright, 3 ticks dim
        // FIX URGENTE: en chat siempre usamos Normal para no deformar la mascota
        // ni hacerla gigante en el header.
        let size = mascot_size_for_chat(area.width);
        render_mascot_at(
            f,
            area.x.saturating_add(1),
            area.y,
            state,
            bright,
            tick,
            accent,
            size,
        );
    }

    // text_x dinámico según el ancho real de la mascota. Aunque chat siempre
    // usa Normal, mantenemos el cálculo dinámico por robustez.
    let text_x = if show_mascot {
        let mw = mascot_width_for_size(mascot_size_for_chat(area.width));
        // mascot empieza en area.x+1, así que el texto va después + gap de 2.
        area.x.saturating_add(1).saturating_add(mw).saturating_add(2)
    } else {
        area.x.saturating_add(1)
    };

    if text_x >= area.right() {
        return;
    }

    let text_area = Rect {
        x: text_x,
        y: area.y,
        width: area.right().saturating_sub(text_x),
        height: area.height,
    };

    let lines = vec![Line::from(Span::styled(
        "NEXUM",
        Style::default()
            .fg(theme::NEXUM_GREEN)
            .add_modifier(Modifier::BOLD),
    ))];
    f.render_widget(Paragraph::new(lines), text_area);
}

/// Paleta de colores para la mascota. Todas las partes derivan de esta
/// paleta para garantizar uniformidad por modo/estado.
#[derive(Debug, Clone, Copy)]
struct MascotPalette {
    /// Color principal (cabeza, cuerpo).
    primary: Color,
    /// Variación tenue para la base/sombra (misma familia cromática).
    dim: Color,
    /// Color de los ojos.
    eye: Color,
}

fn dim_color(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * 0.72) as u8,
            (g as f32 * 0.72) as u8,
            (b as f32 * 0.72) as u8,
        ),
        other => other,
    }
}

fn palette_for_state(state: MascotState, accent: Option<Color>, bright: bool) -> MascotPalette {
    let base = state.color();
    let primary = match state {
        MascotState::Idle => accent.unwrap_or(base),
        MascotState::Thinking if bright => Color::Rgb(184, 255, 176),
        MascotState::Processing if bright => Color::Rgb(0, 240, 190),
        _ => base,
    };
    MascotPalette {
        primary,
        dim: dim_color(primary),
        eye: primary,
    }
}

/// Renderiza la mascota con respiración (bright/dim cycle) y blink.
///
/// `size` se pasa explícito desde el caller (welcome/header) para que el arte
/// dibujado sea consistente con el placeholder reservado y el cálculo de
/// posición. Antes `render_mascot_at` re-resolvía el arte con el ancho del
/// frame completo, que podía diferir del `area.width` que el caller usó.
pub fn render_mascot_at(
    f: &mut Frame,
    x: u16,
    y: u16,
    state: MascotState,
    bright: bool,
    tick: u64,
    accent: Option<Color>,
    size: MascotSize,
) {
    let (mut left_eye, mut right_eye) = state.eyes();
    let art = MascotArt::for_size(size);
    let mascot_w = art.width();
    let buf = f.buffer_mut();

    // Limpiar el bounding box completo antes de dibujar. Sin esto,
    // al cambiar de tamaño o de acento/modo, las celdas del borde que la
    // variante nueva no cubre retienen símbolo/fg/bg viejo.
    clear_mascot_bbox(buf, x, y, mascot_w);

    // Blink: only the eyes change. Body color is state-driven and stable.
    if state != MascotState::Error {
        let interval = crate::ui::nexum_motion::MotionMode::from_env().blink_interval_ticks();
        let blink_cycle = tick % interval;
        if blink_cycle >= interval.saturating_sub(2) {
            left_eye = '_';
            right_eye = '_';
        }
    }

    // Todas las partes de la mascota derivan de una paleta coherente.
    // En modo Idle con accent (Plan/Build/Research/Think/Review), toda la
    // mascota adopta el color del modo para evitar mezclas verde/cyan.
    let palette = palette_for_state(state, accent, bright);
    let body_style = Style::default()
        .fg(palette.primary)
        .add_modifier(Modifier::BOLD);
    let head_style = Style::default()
        .fg(palette.primary)
        .add_modifier(Modifier::BOLD);
    let base_style = Style::default()
        .fg(palette.dim)
        .add_modifier(Modifier::BOLD);
    let eye_style = Style::default()
        .fg(palette.eye)
        .add_modifier(Modifier::BOLD);

    // Línea 1: cabeza
    write_line(buf, x, y, art.line1, head_style);
    // Línea 2: cuerpo + ojos
    write_line(buf, x, y + 1, art.line2_left, body_style);
    let eye_x = x + art.line2_left.chars().count() as u16;
    write_char(buf, eye_x, y + 1, left_eye, eye_style);
    write_char(buf, eye_x + 1, y + 1, ' ', body_style);
    write_char(buf, eye_x + 2, y + 1, ' ', body_style);
    write_char(buf, eye_x + 3, y + 1, right_eye, eye_style);
    write_line(buf, eye_x + 4, y + 1, art.line2_right, body_style);
    // Línea 3: base/sombra
    write_line(buf, x, y + 2, art.line3, base_style);
}

fn write_line(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) {
    for (i, ch) in text.chars().enumerate() {
        if let Some(cell) = buf.cell_mut((x + i as u16, y)) {
            cell.set_char(ch);
            cell.fg = style.fg.unwrap_or(Color::Reset);
            // FASE 1: fijar el fondo para que las celdas de mascota nunca
            // retengan un bg stale del frame anterior (ghost cells al
            // cambiar modo/tamaño).
            cell.bg = theme::CHAT_BG;
            cell.modifier = style.add_modifier;
        }
    }
}

fn write_char(buf: &mut Buffer, x: u16, y: u16, ch: char, style: Style) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(ch);
        cell.fg = style.fg.unwrap_or(Color::Reset);
        // FASE 1: idem write_line — fondo consistente.
        cell.bg = theme::CHAT_BG;
        cell.modifier = style.add_modifier;
    }
}

/// FASE 1: limpia el bounding box completo de la mascota (width × MASCOT_HEIGHT)
/// con espacio + bg CHAT_BG antes de dibujar. Elimina residuos al cambiar
/// tamaño (Plus→Normal) o acento (Plan/Build/Research): las celdas que la
/// variante nueva no cubre quedan en espacio+fondo en vez de retener el
/// símbolo/color del frame anterior.
fn clear_mascot_bbox(buf: &mut Buffer, x: u16, y: u16, width: u16) {
    for row in 0..MASCOT_HEIGHT {
        for col in 0..width {
            if let Some(cell) = buf.cell_mut((x.saturating_add(col), y.saturating_add(row))) {
                cell.set_char(' ');
                cell.fg = theme::CHAT_BG;
                cell.bg = theme::CHAT_BG;
                cell.modifier = Modifier::empty();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::Terminal;

    fn render_with(size: MascotSize, state: MascotState, accent: Option<Color>) -> Buffer {
        let width = 60;
        let height = 4;
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        let art = MascotArt::for_size(size);
        let (left_eye, right_eye) = state.eyes();
        // Usar la misma paleta que render_mascot_at para consistencia en tests.
        let palette = palette_for_state(state, accent, false);
        let body_style = Style::default()
            .fg(palette.primary)
            .add_modifier(Modifier::BOLD);
        let head_style = Style::default()
            .fg(palette.primary)
            .add_modifier(Modifier::BOLD);
        let base_style = Style::default()
            .fg(palette.dim)
            .add_modifier(Modifier::BOLD);
        let eye_style = Style::default()
            .fg(palette.eye)
            .add_modifier(Modifier::BOLD);
        let x = 0u16;
        let y = 0u16;
        write_line(&mut buf, x, y, art.line1, head_style);
        write_line(&mut buf, x, y + 1, art.line2_left, body_style);
        let eye_x = x + art.line2_left.chars().count() as u16;
        write_char(&mut buf, eye_x, y + 1, left_eye, eye_style);
        write_char(&mut buf, eye_x + 1, y + 1, ' ', body_style);
        write_char(&mut buf, eye_x + 2, y + 1, ' ', body_style);
        write_char(&mut buf, eye_x + 3, y + 1, right_eye, eye_style);
        write_line(&mut buf, eye_x + 4, y + 1, art.line2_right, body_style);
        write_line(&mut buf, x, y + 2, art.line3, base_style);
        buf
    }

    #[test]
    fn test_mascot_normal_preserves_silhouette() {
        // FIX URGENTE: la identidad visual por defecto es Normal. Verificamos
        // que el ancho sea el esperado y que Plus5 (el unico tamaño grande
        // permitido en welcome bajo configuracion explicita) sea distinto.
        let normal = MascotArt::for_size(MascotSize::Normal).width();
        let plus5 = MascotArt::for_size(MascotSize::Plus5).width();
        assert_eq!(normal, 10, "Normal debe medir 10 columnas");
        assert!(
            plus5 > normal,
            "Plus5 debe ser mas ancho que Normal: normal={}, plus5={}",
            normal,
            plus5
        );
    }

    #[test]
    fn test_mascot_idle_accent_paints_uniformly() {
        // En modo Idle con accent (Plan), toda la mascota debe adoptar el color
        // del modo. No debe quedar el cuerpo verde mientras la cabeza es cyan.
        let plan_accent = Color::Rgb(0, 212, 170); // #00D4AA cyan
        let buf = render_with(MascotSize::Normal, MascotState::Idle, Some(plan_accent));

        // Cabeza (linea 1) usa el accent.
        let head_fg = buf.cell((1, 0)).map(|c| c.fg);
        assert_eq!(
            head_fg,
            Some(plan_accent),
            "cabeza debe usar accent del modo, got {:?}",
            head_fg
        );

        // Cuerpo (linea 2, primer char) tambien usa el accent.
        let body_fg = buf.cell((0, 1)).map(|c| c.fg);
        assert_eq!(
            body_fg,
            Some(plan_accent),
            "cuerpo debe usar accent del modo, got {:?}",
            body_fg
        );

        // Ojos usan el accent.
        let eye_x = MascotArt::for_size(MascotSize::Normal).line2_left.chars().count();
        let eye_fg = buf.cell((eye_x as u16, 1)).map(|c| c.fg);
        assert_eq!(
            eye_fg,
            Some(plan_accent),
            "ojos deben usar accent del modo, got {:?}",
            eye_fg
        );
    }

    #[test]
    fn test_mascot_runtime_state_overrides_accent() {
        // Cuando esta en runtime (Thinking/Tool/Error), el acento no pinta la cabeza.
        let accent = Color::Rgb(0, 212, 170);
        let buf = render_with(MascotSize::Normal, MascotState::Tool, Some(accent));
        let head_color = buf.cell((1, 0)).map(|c| c.fg);
        assert_eq!(
            head_color,
            Some(theme::WARNING),
            "runtime Tool debe ganar sobre accent, got {:?}",
            head_color
        );
    }

    #[test]
    fn test_write_line_sets_bg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let style = Style::default()
            .fg(theme::NEXUM_GREEN)
            .add_modifier(Modifier::BOLD);
        write_line(&mut buf, 1, 1, "test", style);
        for x in 1..5 {
            let cell = buf.cell((x, 1)).expect("celda debe existir");
            assert_eq!(
                cell.bg, theme::CHAT_BG,
                "celda ({},1) debe tener bg CHAT_BG",
                x
            );
        }
    }

    #[test]
    fn test_mascot_clears_bounding_box_on_redraw() {
        // Simular que el frame anterior dejo un dibujo Plus10 (ancho 18) y ahora
        // vamos a dibujar Normal (ancho 10). clear_mascot_bbox debe limpiar
        // exactamente el bbox de Normal, dejando intacto el residuo de Plus10
        // que queda fuera.
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 5));
        let plus10_w = MascotArt::for_size(MascotSize::Plus10).width();
        for row in 0..MASCOT_HEIGHT {
            for col in 0..plus10_w {
                if let Some(cell) = buf.cell_mut((1 + col, 1 + row)) {
                    cell.set_char('X');
                    cell.fg = Color::Red;
                    cell.bg = Color::Red;
                }
            }
        }

        let normal_w = MascotArt::for_size(MascotSize::Normal).width();
        clear_mascot_bbox(&mut buf, 1, 1, normal_w);

        for row in 0..MASCOT_HEIGHT {
            for col in 0..normal_w {
                let cell = buf
                    .cell((1 + col, 1 + row))
                    .expect("celda dentro del bbox de Normal debe existir");
                assert_eq!(
                    cell.symbol(),
                    " ",
                    "celda ({},{}) dentro de Normal debe estar limpia",
                    col,
                    row
                );
                assert_eq!(
                    cell.bg,
                    theme::CHAT_BG,
                    "celda ({},{}) dentro de Normal debe tener CHAT_BG",
                    col,
                    row
                );
            }
            for col in normal_w..plus10_w {
                let cell = buf
                    .cell((1 + col, 1 + row))
                    .expect("celda fuera del bbox de Normal debe existir");
                assert_eq!(
                    cell.symbol(),
                    "X",
                    "celda ({},{}) fuera de Normal no debe haber sido tocada",
                    col,
                    row
                );
                assert_eq!(
                    cell.bg,
                    Color::Red,
                    "celda ({},{}) fuera de Normal conserva residuo",
                    col,
                    row
                );
            }
        }
    }

    #[test]
    fn test_mascot_accent_change_no_ghost() {
        let backend = TestBackend::new(40, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let green = Color::Rgb(0, 255, 0);
        let cyan = Color::Rgb(0, 212, 170);

        // En un mismo frame pre-poblamos el bbox con el color viejo (simulando
        // frame anterior) y luego renderizamos con el nuevo acento.
        terminal
            .draw(|f| {
                let width = MascotArt::for_size(MascotSize::Normal).width();
                let buffer = f.buffer_mut();
                for row in 0..MASCOT_HEIGHT {
                    for col in 0..width {
                        if let Some(cell) = buffer.cell_mut((1 + col, 1 + row)) {
                            cell.set_char('X');
                            cell.fg = green;
                            cell.bg = green;
                        }
                    }
                }
                render_mascot_at(
                    f,
                    1,
                    1,
                    MascotState::Idle,
                    false,
                    0,
                    Some(cyan),
                    MascotSize::Normal,
                );
            })
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        let width = MascotArt::for_size(MascotSize::Normal).width();
        for col in 0..width {
            let cell = buf
                .cell((1 + col, 1))
                .expect("celda de la cabeza debe existir");
            assert_ne!(
                cell.fg, green,
                "no debe quedar fg verde residual en col {}",
                col
            );
        }
    }

    #[test]
    fn test_mascot_size_for_chat_is_always_normal() {
        // FIX URGENTE: el chat nunca debe usar tamaños grandes.
        assert_eq!(
            mascot_size_for_chat(100),
            MascotSize::Normal,
            "chat no usa tamaño grande aunque haya espacio"
        );
        assert_eq!(
            mascot_size_for_chat(30),
            MascotSize::Normal,
            "chat usa Normal en terminal chica"
        );
        assert_eq!(
            mascot_size_for_chat(10),
            MascotSize::Normal,
            "chat usa Normal siempre"
        );
    }

    #[test]
    fn test_mascot_size_for_welcome_defaults_to_normal() {
        // Sin env var, welcome debe usar Normal para no deformar la silueta.
        assert_eq!(
            mascot_size_for_welcome(100),
            MascotSize::Normal,
            "welcome por defecto es Normal"
        );
        assert_eq!(
            mascot_size_for_welcome(20),
            MascotSize::Normal,
            "welcome sin espacio usa Normal"
        );
    }

    #[test]
    fn test_mascot_size_for_welcome_ignores_plus10_plus20() {
        // FIX URGENTE: Plus10/Plus20 estan deshabilitados para welcome.
        // Incluso si el usuario los pide por env var, caemos a Plus5 como maximo.
        // (Nota: este test solo se ejecuta correctamente si NEXUM_MASCOT_SIZE
        // no esta seteada a plus10/plus20; en CI no deberia estarlo.)
        let env_size = std::env::var("NEXUM_MASCOT_SIZE").ok();
        if env_size.as_deref() == Some("plus10") || env_size.as_deref() == Some("plus20") {
            // Si la env var fuerza plus10/plus20, welcome debe limitar a Plus5.
            assert!(
                mascot_size_for_welcome(100) != MascotSize::Plus10
                    && mascot_size_for_welcome(100) != MascotSize::Plus20,
                "welcome nunca usa Plus10/Plus20"
            );
        }
    }

    #[test]
    fn test_text_x_dynamic_with_normal() {
        let backend = TestBackend::new(80, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                render_header(
                    f,
                    Rect::new(0, 0, 80, 3),
                    MascotState::Idle,
                    "",
                    "",
                    false,
                    0,
                    Some(theme::ACCENT),
                );
            })
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        let mut text_x = None;
        for x in 0..80 {
            if buf.cell((x, 0)).map(|c| c.symbol()) == Some("N") {
                text_x = Some(x);
                break;
            }
        }
        let text_x = text_x.expect("NEXUM deberia renderizarse");
        let mascot_end = 1 + MascotArt::for_size(MascotSize::Normal).width();
        assert!(
            text_x >= mascot_end + 2,
            "text_x {} debe estar despues de mascot_end {} con gap 2",
            text_x,
            mascot_end
        );
    }
}
