//! Welcome Card — empty chat brand surface.
//!
//! UX FIX 05.2: exact mascot/logo alignment, stable mascot color, and logo shine sweep.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};

use crate::{
    app::App,
    ui::{nexum_mascot, nexum_motion, theme},
};

const LOGO: &[&str] = &[
    "███╗   ██╗███████╗██╗  ██╗██╗   ██╗███╗   ███╗",
    "████╗  ██║██╔════╝╚██╗██╔╝██║   ██║████╗ ████║",
    "██╔██╗ ██║█████╗   ╚███╔╝ ██║   ██║██╔████╔██║",
    "██║╚██╗██║██╔══╝   ██╔██╗ ██║   ██║██║╚██╔╝██║",
    "██║ ╚████║███████╗██╔╝ ██╗╚██████╔╝██║ ╚═╝ ██║",
    "╚═╝  ╚═══╝╚══════╝╚═╝  ╚═╝ ╚═════╝ ╚═╝     ╚═╝",
];

const LOGO_WIDTH: u16 = 46;
const NARROW_THRESHOLD: u16 = LOGO_WIDTH + 4;
const SHINE_WIDTH: i16 = 4;

const CAPABILITIES: &[&str] = &[
    "◇ Desarrollar código, refactorizar y corregir",
    "◇ Explorar arquitectura y navegar el codebase",
    "◇ Generar documentación, reportes y notas",
    "◇ Analizar logs, errores y calidad de código",
    "◇ Coordinar contexto y orquestar agentes",
    "◇ Ejecutar flujos con control de aprobación",
];

pub(crate) fn render_welcome(f: &mut Frame, app: &App, area: Rect) {
    let narrow = area.width < NARROW_THRESHOLD;
    let tick = app.global_ui.visual_tick.get();
    let motion_mode = nexum_motion::MotionMode::from_env();
    let mut lines: Vec<Line<'static>> = Vec::new();

    if !narrow {
        lines.push(Line::from(""));

        // Mascot placeholder. Actual mascot is rendered after the paragraph so
        // it can be aligned to the logo bounding box, not guessed by viewport.
        for _ in 0..nexum_mascot::MASCOT_HEIGHT {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(""));

        for row in LOGO {
            lines.push(render_logo_row(row, tick, motion_mode));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "Nexum",
            Style::default()
                .fg(theme::NEXUM_GREEN)
                .add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "AI Operating System Personal",
        Style::default().fg(theme::MUTED),
    )));

    // Provider · modelo desde runtime_identity() — fuente única (Sprint A,
    // Bug 5). Antes usaba la copia stale de services.
    let (identity_family, identity_model) = {
        let cfg = app.services.nexum_config.read();
        crate::app::runtime_identity::statusbar_identity(&cfg)
    };
    let provider = crate::ui::display_provider_name(&identity_family);
    let model = identity_model.as_str();
    if !provider.is_empty() || !model.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(provider, Style::default().fg(theme::THINKING)),
            Span::styled(" · ", Style::default().fg(theme::MUTED)),
            Span::styled(model.to_string(), Style::default().fg(theme::MODEL_INFO)),
        ]));
    }

    let cap_idx = if motion_mode == nexum_motion::MotionMode::Off {
        0
    } else {
        let interval = motion_mode.carousel_interval_ticks();
        ((tick / interval) as usize) % CAPABILITIES.len()
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", CAPABILITIES[cap_idx]),
        Style::default().fg(theme::ACCENT),
    )));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" /model", Style::default().fg(theme::WARNING)),
        Span::styled("  ", Style::default().fg(theme::MUTED)),
        Span::styled("/agents", Style::default().fg(theme::WARNING)),
        Span::styled("  ", Style::default().fg(theme::MUTED)),
        Span::styled("/help", Style::default().fg(theme::WARNING)),
    ]));

    if app.services.provider_name.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Nexum no tiene provider configurado aún.",
            Style::default().fg(theme::MUTED),
        )));
        lines.push(Line::from(Span::styled(
            "Ejecutá: /model para agregar un provider, o /help para ver comandos.",
            Style::default().fg(theme::MUTED),
        )));
    }

    let content_height = lines.len() as u16;
    let padding_top = area.height.saturating_sub(content_height) / 3;
    let centered_lines: Vec<Line<'static>> = lines.into_iter().map(|l| l.centered()).collect();
    let mut padded_lines: Vec<Line<'static>> = (0..padding_top).map(|_| Line::from("")).collect();
    padded_lines.extend(centered_lines);

    f.render_widget(Paragraph::new(Text::from(padded_lines)), area);

    if !narrow {
        let logo_x = area.x + area.width.saturating_sub(LOGO_WIDTH) / 2;
        // FIX URGENTE: resolver size de welcome una sola vez. Por defecto
        // Normal para conservar la silueta; Plus5 si el usuario lo pidió y
        // cabe. Plus10/Plus20 nunca se usan porque deforman la mascota.
        let size = nexum_mascot::mascot_size_for_welcome(area.width);
        let mascot_width = nexum_mascot::mascot_width_for_size(size);
        let mascot_x = logo_x + LOGO_WIDTH.saturating_sub(mascot_width) / 2;
        let mascot_y = area.y + padding_top + 1;
        let bright = motion_mode.is_enabled() && (tick / 15) % 6 < 3;
        let state = if app.services.provider_name.is_empty() {
            nexum_mascot::MascotState::Offline
        } else {
            nexum_mascot::MascotState::Idle
        };
        nexum_mascot::render_mascot_at(
            f,
            mascot_x,
            mascot_y,
            state,
            bright,
            tick,
            Some(agent_mode_accent(app.global_ui.agent_mode)),
            size,
        );
    }
}

fn agent_mode_accent(mode: crate::app::AgentMode) -> Color {
    match mode {
        crate::app::AgentMode::Build => theme::NEXUM_GREEN,
        crate::app::AgentMode::Plan => theme::ACCENT,
        crate::app::AgentMode::Think => theme::THINKING,
        crate::app::AgentMode::Review => theme::WARNING,
        crate::app::AgentMode::Research => Color::Rgb(96, 165, 250),
    }
}

fn render_logo_row(row: &str, tick: u64, motion_mode: nexum_motion::MotionMode) -> Line<'static> {
    let shine_x = shine_column(tick, motion_mode);
    let spans = row
        .chars()
        .enumerate()
        .map(|(idx, ch)| {
            let color = logo_color_for_column(idx as i16, shine_x);
            Span::styled(
                ch.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn shine_column(tick: u64, motion_mode: nexum_motion::MotionMode) -> Option<i16> {
    if !nexum_motion::logo_shine_enabled(motion_mode) {
        return None;
    }
    let interval = std::env::var("NEXUM_LOGO_SHINE_INTERVAL")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(10))
        .unwrap_or_else(|| motion_mode.shine_interval_ticks());
    let duration = motion_mode.shine_duration_ticks().max(1);
    let phase = tick % interval.max(duration + 1);
    if phase >= duration {
        return None;
    }
    let span = LOGO_WIDTH as i16 + SHINE_WIDTH * 2;
    let progress = phase as f32 / duration as f32;
    Some((-SHINE_WIDTH as f32 + progress * span as f32) as i16)
}

fn logo_color_for_column(col: i16, shine_x: Option<i16>) -> Color {
    let Some(shine_x) = shine_x else {
        return theme::NEXUM_GREEN;
    };
    let distance = (col - shine_x).abs();
    if distance <= 1 {
        Color::Rgb(241, 245, 249) // shine core
    } else if distance <= SHINE_WIDTH {
        theme::ACCENT
    } else {
        theme::NEXUM_GREEN
    }
}
