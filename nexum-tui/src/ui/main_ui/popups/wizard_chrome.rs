//! Chrome compartido del wizard de primer arranque.
//!
//! Antes cada pantalla armaba su propio `BorderedPanel` con un título y una
//! lista donde **todo pesaba igual**: mismo color, mismo tamaño, sin separación,
//! y las opciones chocando contra el footer. No parecía Nexum.
//!
//! Acá vive la identidad: la mascota arriba, el logo, la jerarquía
//! título/contexto/opciones/footer, y el degradado por altura. Las pantallas
//! sólo aportan su contenido.
//!
//! **Regla dura del degradado:** nunca cortar el robot a la mitad, y nunca
//! dejar que el arte empuje las opciones fuera de vista. Ante la duda entre
//! mostrar el arte o mostrar las opciones, **ganan las opciones** — el wizard
//! tiene que ser usable en una terminal de 80×24. Por eso se mide contra el
//! alto disponible ANTES de decidir qué dibujar.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};

use crate::ui::{nexum_mascot, theme};

/// Logo ASCII de NEXUM, el mismo del welcome card.
const LOGO: &[&str] = &[
    "███╗   ██╗███████╗██╗  ██╗██╗   ██╗███╗   ███╗",
    "████╗  ██║██╔════╝╚██╗██╔╝██║   ██║████╗ ████║",
    "██╔██╗ ██║█████╗   ╚███╔╝ ██║   ██║██╔████╔██║",
    "██║╚██╗██║██╔══╝   ██╔██╗ ██║   ██║██║╚██╔╝██║",
    "██║ ╚████║███████╗██╔╝ ██╗╚██████╔╝██║ ╚═╝ ██║",
    "╚═╝  ╚═══╝╚══════╝╚═╝  ╚═╝ ╚═════╝ ╚═╝     ╚═╝",
];

const LOGO_WIDTH: u16 = 46;
/// Alto del bloque de arte completo: aire + mascota + aire + logo + aire.
const ALTO_ARTE_COMPLETO: u16 = 1 + nexum_mascot::MASCOT_HEIGHT + 1 + 6 + 1;
/// Alto del arte sin la mascota: logo + aire.
const ALTO_ARTE_LOGO: u16 = 6 + 1;
/// Alto del arte mínimo: una línea de título + aire.
const ALTO_ARTE_TITULO: u16 = 2;
/// Piso de contenido que el arte no puede invadir.
///
/// Título (1) + contexto (1) + aire (1) + tres opciones con su detalle y su
/// separación (9) + regla y teclas (2) = 14. Ése es el piso, sin redondear para
/// abajo: recortarlo hace que la pantalla "entre" pero sin una sola línea de
/// respiro, que es la queja original ("las opciones chocan contra el footer").
/// En 80×24 esto hace que el robot degrade a sólo-logo, y está bien: ganan las
/// opciones.
const CONTENIDO_MINIMO: u16 = 14;

/// Qué nivel del degradado entra en el alto disponible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NivelArte {
    /// Robot pixel-art + logo ASCII.
    RobotYLogo,
    /// Sólo el logo ASCII (el robot no entra).
    SoloLogo,
    /// "NEXUM" en texto plano (tampoco entra el logo).
    SoloTitulo,
}

impl NivelArte {
    fn alto(self) -> u16 {
        match self {
            Self::RobotYLogo => ALTO_ARTE_COMPLETO,
            Self::SoloLogo => ALTO_ARTE_LOGO,
            Self::SoloTitulo => ALTO_ARTE_TITULO,
        }
    }
}

/// Elige el nivel del degradado midiendo ANTES de dibujar.
///
/// Se decide por alto y por ancho: el logo mide 46 columnas y en una terminal
/// más angosta se vería cortado, que es peor que no mostrarlo.
pub(crate) fn nivel_para(area: Rect) -> NivelArte {
    let cabe = |nivel: NivelArte| {
        area.height >= nivel.alto().saturating_add(CONTENIDO_MINIMO)
            && (nivel == NivelArte::SoloTitulo || area.width >= LOGO_WIDTH + 4)
    };
    if cabe(NivelArte::RobotYLogo) {
        NivelArte::RobotYLogo
    } else if cabe(NivelArte::SoloLogo) {
        NivelArte::SoloLogo
    } else {
        NivelArte::SoloTitulo
    }
}

/// Dibuja el encabezado de marca y devuelve el área que queda para el contenido.
///
/// La mascota se renderiza aparte del `Paragraph` porque necesita alinearse a la
/// caja del logo, igual que en el welcome card: es el mismo asset y la misma
/// rutina, ya resuelta y probada ahí.
pub(crate) fn render_encabezado(f: &mut Frame, area: Rect, tick: u64) -> Rect {
    let nivel = nivel_para(area);
    let mut lines: Vec<Line<'static>> = Vec::new();

    match nivel {
        NivelArte::RobotYLogo => {
            lines.push(Line::from(""));
            // Hueco para la mascota, que se dibuja después sobre estas filas.
            for _ in 0..nexum_mascot::MASCOT_HEIGHT {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(""));
            for row in LOGO {
                lines.push(linea_logo(row));
            }
            lines.push(Line::from(""));
        }
        NivelArte::SoloLogo => {
            for row in LOGO {
                lines.push(linea_logo(row));
            }
            lines.push(Line::from(""));
        }
        NivelArte::SoloTitulo => {
            lines.push(Line::from(Span::styled(
                "NEXUM",
                Style::default()
                    .fg(theme::NEXUM_GREEN)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
        }
    }

    let alto = lines.len() as u16;
    let centradas: Vec<Line<'static>> = lines.into_iter().map(|l| l.centered()).collect();
    let zona_arte = Rect {
        height: alto.min(area.height),
        ..area
    };
    f.render_widget(Paragraph::new(Text::from(centradas)), zona_arte);

    if nivel == NivelArte::RobotYLogo {
        let logo_x = area.x + area.width.saturating_sub(LOGO_WIDTH) / 2;
        let size = nexum_mascot::mascot_size_for_welcome(area.width);
        let mascot_width = nexum_mascot::mascot_width_for_size(size);
        let mascot_x = logo_x + LOGO_WIDTH.saturating_sub(mascot_width) / 2;
        nexum_mascot::render_mascot_at(
            f,
            mascot_x,
            area.y + 1,
            nexum_mascot::MascotState::Idle,
            false,
            tick,
            Some(theme::NEXUM_GREEN),
            size,
        );
    }

    // El contenido arranca alineado con el borde izquierdo del logo, no contra
    // el margen: con el arte centrado y el texto pegado a la izquierda, las dos
    // mitades de la pantalla se leen como si fueran de dos diseños distintos.
    let sangria = match nivel {
        NivelArte::SoloTitulo => 0,
        _ => area.width.saturating_sub(LOGO_WIDTH) / 2,
    };
    Rect {
        x: area.x.saturating_add(sangria),
        y: area.y.saturating_add(alto),
        width: area.width.saturating_sub(sangria),
        height: area.height.saturating_sub(alto),
    }
}

fn linea_logo(row: &str) -> Line<'static> {
    Line::from(Span::styled(
        row.to_string(),
        Style::default()
            .fg(theme::NEXUM_GREEN)
            .add_modifier(Modifier::BOLD),
    ))
}

/// Título de pantalla + una línea de contexto que dice por qué importa.
///
/// La jerarquía es el punto: el título en verde bold dice **qué es esto**, el
/// contexto en `MUTED` dice **por qué importa**. Antes las dos líneas tenían el
/// mismo peso y la pantalla se leía como una lista sin encabezado.
pub(crate) fn lineas_titulo(titulo: &str, contexto: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        titulo.to_string(),
        Style::default()
            .fg(theme::NEXUM_GREEN)
            .add_modifier(Modifier::BOLD),
    ))];
    if !contexto.is_empty() {
        lines.push(Line::from(Span::styled(
            contexto.to_string(),
            Style::default().fg(theme::MUTED),
        )));
    }
    // Aire entre el encabezado y las opciones: antes se tocaban.
    lines.push(Line::from(""));
    lines
}

/// Una opción de una lista, alineada en la misma columna que sus hermanas.
///
/// El marcador es ASCII (`>`), no un glifo: una terminal sin la fuente adecuada
/// dibuja un cuadradito, y un cuadradito al lado de la opción seleccionada es
/// exactamente el tipo de detalle que hace que algo no parezca terminado.
pub(crate) fn linea_opcion(seleccionada: bool, etiqueta: &str) -> Line<'static> {
    let marcador = if seleccionada { "  > " } else { "    " };
    let estilo = if seleccionada {
        Style::default()
            .fg(theme::THINKING)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::TEXT)
    };
    Line::from(vec![
        Span::styled(marcador, Style::default().fg(theme::THINKING)),
        Span::styled(etiqueta.to_string(), estilo),
    ])
}

/// Segunda línea de una opción: la descripción, sangrada bajo su etiqueta.
pub(crate) fn linea_detalle(seleccionada: bool, detalle: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("      ", Style::default()),
        Span::styled(
            detalle.to_string(),
            Style::default().fg(if seleccionada {
                theme::THINKING
            } else {
                theme::MUTED
            }),
        ),
    ])
}

/// Footer de teclas, anclado abajo y separado por una regla.
///
/// Va anclado al fondo del área y no al final de la lista: antes las opciones
/// chocaban contra las teclas y no se sabía dónde terminaba una cosa y empezaba
/// la otra.
pub(crate) fn render_footer(f: &mut Frame, area: Rect, teclas: &[(&str, String)]) {
    if area.height < 2 {
        return;
    }
    let ancho_regla = area.width.saturating_sub(4).min(60) as usize;
    let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
    for (i, (tecla, accion)) in teclas.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(theme::DIM)));
        }
        spans.push(Span::styled(
            tecla.to_string(),
            Style::default()
                .fg(theme::THINKING)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {accion}"),
            Style::default().fg(theme::MUTED),
        ));
    }

    let lines = vec![
        Line::from(Span::styled(
            format!("  {}", "─".repeat(ancho_regla)),
            Style::default().fg(theme::DIM),
        )),
        Line::from(spans),
    ];
    let zona = Rect {
        y: area.y + area.height - 2,
        height: 2,
        ..area
    };
    f.render_widget(Paragraph::new(Text::from(lines)), zona);
}

/// Área para el cuerpo, dejando libre el footer.
pub(crate) fn zona_cuerpo(area: Rect) -> Rect {
    Rect {
        height: area.height.saturating_sub(3),
        ..area
    }
}

#[cfg(test)]
#[path = "wizard_chrome_test.rs"]
mod tests;
