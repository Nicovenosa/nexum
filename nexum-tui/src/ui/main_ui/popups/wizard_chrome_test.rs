//! Tests del chrome del wizard.
//!
//! El criterio que fijan no es estético: es que el wizard siga siendo **usable**
//! cuando la terminal es chica. Si el arte empuja las opciones fuera de vista,
//! el usuario no puede completar el primer arranque.

use super::*;

fn area(w: u16, h: u16) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: w,
        height: h,
    }
}

#[test]
fn en_una_terminal_grande_entra_el_robot() {
    assert_eq!(nivel_para(area(120, 40)), NivelArte::RobotYLogo);
}

#[test]
fn en_80x24_las_opciones_ganan_sobre_el_arte() {
    // El tamaño clásico de terminal es el caso de prueba, no el borde raro.
    // Con 24 filas el bloque completo (12) más el piso de contenido (12) no
    // entra, así que degrada en vez de comerse el aire de las opciones.
    let nivel = nivel_para(area(80, 24));
    assert_ne!(
        nivel,
        NivelArte::RobotYLogo,
        "el robot no puede empujar las opciones fuera de vista en 80x24"
    );
}

#[test]
fn en_una_terminal_angosta_no_se_dibuja_el_logo_cortado() {
    // El logo mide 46 columnas: mostrarlo cortado es peor que no mostrarlo.
    assert_eq!(nivel_para(area(40, 40)), NivelArte::SoloTitulo);
}

#[test]
fn con_muy_pocas_filas_queda_solo_el_titulo() {
    assert_eq!(nivel_para(area(120, 14)), NivelArte::SoloTitulo);
}

#[test]
fn el_degradado_es_monotono_en_altura() {
    // A más alto disponible, nunca menos arte. Un degradado que se invierte en
    // algún punto es un bug que sólo se ve redimensionando.
    let peso = |n: NivelArte| match n {
        NivelArte::SoloTitulo => 0,
        NivelArte::SoloLogo => 1,
        NivelArte::RobotYLogo => 2,
    };
    let mut anterior = 0;
    for h in 10..60 {
        let actual = peso(nivel_para(area(120, h)));
        assert!(
            actual >= anterior,
            "en alto {h} el arte bajó de nivel al crecer la pantalla"
        );
        anterior = actual;
    }
}

#[test]
fn el_encabezado_nunca_devuelve_un_area_fuera_del_original() {
    // `render_encabezado` recorta el área para el contenido: si devolviera algo
    // más alto que lo que recibió, el contenido se dibujaría fuera de pantalla.
    for (w, h) in [(120u16, 40u16), (80, 24), (40, 12), (20, 8)] {
        let a = area(w, h);
        let nivel = nivel_para(a);
        assert!(
            nivel.alto() <= h || nivel == NivelArte::SoloTitulo,
            "el nivel elegido para {w}x{h} no entra"
        );
    }
}

#[test]
fn el_marcador_de_seleccion_es_ascii() {
    // Un glifo que la terminal no puede dibujar sale como cuadradito, y un
    // cuadradito al lado de la opción activa es exactamente lo que hace que
    // algo no parezca terminado.
    let linea = linea_opcion(true, "Opción");
    let texto: String = linea
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>();
    assert!(texto.contains('>'), "el marcador es '>': {texto}");
    assert!(
        texto.chars().all(|c| c.is_ascii() || c.is_alphabetic()),
        "sin glifos decorativos fuera de ASCII: {texto}"
    );
}

#[test]
fn las_opciones_se_alinean_entre_si() {
    let sel: String = linea_opcion(true, "A")
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    let no_sel: String = linea_opcion(false, "A")
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(
        sel.len(),
        no_sel.len(),
        "seleccionada y no seleccionada deben ocupar lo mismo, o la lista salta"
    );
}

#[test]
fn el_titulo_y_el_contexto_no_pesan_igual() {
    let lineas = lineas_titulo("Elegí tu idioma", "Todo lo demás va a seguirlo");
    let titulo = &lineas[0].spans[0];
    let contexto = &lineas[1].spans[0];
    assert!(
        titulo.style.add_modifier.contains(Modifier::BOLD),
        "el título va en bold"
    );
    assert!(
        !contexto.style.add_modifier.contains(Modifier::BOLD),
        "el contexto NO va en bold: si todo pesa igual, no hay jerarquía"
    );
    assert_ne!(titulo.style.fg, contexto.style.fg);
}

#[test]
fn hay_aire_despues_del_encabezado() {
    let lineas = lineas_titulo("T", "C");
    let ultima: String = lineas
        .last()
        .unwrap()
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        ultima.trim().is_empty(),
        "sin una línea en blanco, las opciones chocan contra el título"
    );
}
