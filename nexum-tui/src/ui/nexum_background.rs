//! Nexum background — campo de estrellas estilo hormiguero
//!
//! Fondo animado sutil con estrellas y meteoros tipo MiMo.
//! Control via NEXUM_BG_MODE=welcome|subtle|full|off (default: welcome).
//! No tapa contenido (se renderiza como base layer).

use ratatui::{
    style::{Color, Modifier, Style},
    Frame,
};

use crate::ui::{nexum_motion, theme};

/// Densidad de estrellas en welcome (1 de cada N celdas tiene estrella).
const WELCOME_STAR_DENSITY: u32 = 280;

/// Densidad de estrellas en modo subtle, para chat legible.
const SUBTLE_STAR_DENSITY: u32 = 800;

/// El header persistente de Nexum ocupa 3 filas y debe quedar limpio.
const HEADER_RESERVED_ROWS: u16 = 3;

/// Frames de twinkle antes de recalcular
const TWINKLE_INTERVAL: u64 = 30;

const METEOR_DURATION: u64 = 16;
const METEOR_TRAIL: u16 = 5;

/// Renderiza el fondo: sólido + opcionalmente estrellas.
/// Debe llamarse ANTES de renderizar el contenido principal.
pub fn render_background(f: &mut Frame, tick: u64, is_welcome: bool) {
    let mode = BackgroundMode::from_env();
    let motion_mode = nexum_motion::MotionMode::from_env();
    let area = f.area();
    let buf = f.buffer_mut();

    // SIEMPRE pintar fondo sólido en todas las celdas (UX FIX 04)
    let solid_color = theme::CHAT_BG;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.bg = solid_color;
            }
        }
    }

    // Estrellas: solo en welcome por default; subtle/full habilitan chat con baja intensidad.
    let density = match mode {
        BackgroundMode::Off => return,
        BackgroundMode::Welcome if !is_welcome => return,
        BackgroundMode::Welcome => density_from_env(WELCOME_STAR_DENSITY),
        BackgroundMode::Subtle if is_welcome => density_from_env(WELCOME_STAR_DENSITY),
        BackgroundMode::Subtle => density_from_env(SUBTLE_STAR_DENSITY),
        BackgroundMode::Full if is_welcome => density_from_env(220),
        BackgroundMode::Full => density_from_env(650),
    };

    if density == 0 {
        return;
    }

    let phase = if motion_mode.is_enabled() {
        (tick / TWINKLE_INTERVAL) as u32
    } else {
        0
    };

    let star_dim = Color::Rgb(51, 65, 85); // #334155
    let star_mid = Color::Rgb(100, 116, 139); // #64748B
    let star_green_dim = Color::Rgb(11, 61, 26); // #0B3D1A
    let star_green = Color::Rgb(23, 138, 58); // #178A3A
    let star_bright = Color::Rgb(241, 245, 249); // #F1F5F9

    for y in area.top().saturating_add(HEADER_RESERVED_ROWS)..area.bottom() {
        for x in area.left()..area.right() {
            let h = hash(x, y);
            if h % density != 0 {
                continue;
            }

            // Twinkle: pocas estrellas brillantes, más profundidad gris/verde sutil.
            let twinkle_h = hash(x, y).wrapping_add(phase);
            let is_twinkling = motion_mode.is_enabled() && twinkle_h % 9 == 0;

            let (char, style) = if is_twinkling {
                (Sym::Bright, Style::default().fg(star_bright))
            } else if h % 11 == 0 {
                (Sym::Medium, Style::default().fg(star_green))
            } else if h % 4 == 0 {
                (Sym::Dim, Style::default().fg(star_mid))
            } else if h % 3 == 0 {
                (Sym::Tiny, Style::default().fg(star_dim))
            } else {
                (Sym::Tiny, Style::default().fg(star_green_dim))
            };

            if let Some(cell) = buf.cell_mut((x, y)) {
                // Solo escribir si la celda está vacía (no pisar contenido)
                if cell.symbol() == " " || cell.symbol().is_empty() {
                    cell.set_char(char.as_char());
                    cell.fg = style.fg.unwrap_or(Color::Reset);
                    cell.modifier = Modifier::empty();
                }
            }
        }
    }

    if is_welcome && nexum_motion::meteors_enabled(motion_mode) {
        render_meteor(f, tick, area, motion_mode);
    }
}

fn density_from_env(default_density: u32) -> u32 {
    match std::env::var("NEXUM_BG_DENSITY") {
        Ok(v) if v.eq_ignore_ascii_case("low") => {
            default_density.saturating_add(default_density / 2)
        }
        Ok(v) if v.eq_ignore_ascii_case("high") => {
            default_density.saturating_sub(default_density / 4).max(80)
        }
        _ => default_density,
    }
}

/// Hash pseudo-aleatorio determinístico para una posición (x, y).
fn hash(x: u16, y: u16) -> u32 {
    let mut h = (x as u32).wrapping_mul(374761393);
    h = h.wrapping_add((y as u32).wrapping_mul(668265263));
    h = h ^ (h >> 13);
    h = h.wrapping_mul(1274126177);
    h ^ (h >> 16)
}

/// Hash from a single u64 (for starfall seed).
fn hash_u64(mut v: u64) -> u64 {
    v = v.wrapping_mul(0xbf58476d1ce4e5b9);
    v ^= v >> 27;
    v = v.wrapping_mul(0x94d049bb133111eb);
    v ^ (v >> 31)
}

fn render_meteor(
    f: &mut Frame,
    tick: u64,
    area: ratatui::layout::Rect,
    motion_mode: nexum_motion::MotionMode,
) {
    let buf = f.buffer_mut();
    let interval = meteor_interval(motion_mode);
    let comet_cycle = tick / interval;
    let comet_phase = tick % interval;

    if comet_phase >= METEOR_DURATION {
        return;
    }

    let seed = hash_u64(comet_cycle);
    let travel = (area.width / 3).clamp(18, 42);
    let left_lane = seed % 2 == 0;
    let start_x = if left_lane {
        area.left().saturating_add(3 + (seed % 12) as u16)
    } else {
        area.right().saturating_sub(travel + 8 + (seed % 10) as u16)
    };
    let start_y = area
        .top()
        .saturating_add(HEADER_RESERVED_ROWS + 1 + (seed % 3) as u16);

    let progress = comet_phase as f32 / METEOR_DURATION as f32;
    let comet_x = start_x + (progress * travel as f32) as u16;
    let comet_y = start_y + (progress * 10.0) as u16;

    write_meteor_cell(buf, area, comet_x, comet_y, '✦', Color::Rgb(241, 245, 249));

    for i in 1..=METEOR_TRAIL {
        let trail_x = comet_x.saturating_sub(i);
        let trail_y = comet_y.saturating_sub(i);
        let color = match i {
            1 => Color::Rgb(0, 212, 170),
            2 => Color::Rgb(23, 138, 58),
            _ => Color::Rgb(51, 65, 85),
        };
        write_meteor_cell(buf, area, trail_x, trail_y, '·', color);
    }
}

fn meteor_interval(motion_mode: nexum_motion::MotionMode) -> u64 {
    let base = motion_mode.meteor_interval_ticks();
    match std::env::var("NEXUM_METEOR_DENSITY") {
        Ok(v) if v.eq_ignore_ascii_case("high") => base.saturating_sub(base / 3).max(60),
        Ok(v) if v.eq_ignore_ascii_case("medium") => base,
        _ => base.saturating_add(base / 2),
    }
}

fn write_meteor_cell(
    buf: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    x: u16,
    y: u16,
    ch: char,
    color: Color,
) {
    if x >= area.right()
        || y >= area.bottom()
        || y < area.top().saturating_add(HEADER_RESERVED_ROWS)
    {
        return;
    }
    if in_center_focus_zone(area, x, y) {
        return;
    }
    if let Some(cell) = buf.cell_mut((x, y)) {
        if cell.symbol() == " " || cell.symbol().is_empty() {
            cell.set_char(ch);
            cell.fg = color;
            cell.modifier = Modifier::empty();
        }
    }
}

fn in_center_focus_zone(area: ratatui::layout::Rect, x: u16, y: u16) -> bool {
    let center_x = area.left() + area.width / 2;
    let protected_left = center_x.saturating_sub(32);
    let protected_right = center_x.saturating_add(32);
    let protected_top = area.top().saturating_add(5);
    let protected_bottom = area.top().saturating_add(20).min(area.bottom());
    x >= protected_left && x <= protected_right && y >= protected_top && y <= protected_bottom
}

#[derive(Clone, Copy)]
enum Sym {
    Bright,
    Medium,
    Dim,
    Tiny,
}

impl Sym {
    fn as_char(&self) -> char {
        match self {
            Sym::Bright => '✶',
            Sym::Medium => '✦',
            Sym::Dim => '✧',
            Sym::Tiny => '·',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundMode {
    Welcome,
    Subtle,
    Full,
    Off,
}

impl BackgroundMode {
    fn from_env() -> Self {
        match std::env::var("NEXUM_BG_MODE") {
            Ok(v) if v.eq_ignore_ascii_case("off") => Self::Off,
            Ok(v) if v.eq_ignore_ascii_case("full") => Self::Full,
            Ok(v) if v.eq_ignore_ascii_case("subtle") => Self::Subtle,
            Ok(_) => Self::Welcome,
            Err(_) => Self::Welcome,
        }
    }
}
