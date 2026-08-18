//! Estos tests existen para probar que Doctor **FALLA**, no que pasa.
//!
//! Un Doctor que incorpora gates pero nunca los vio fallar no prueba nada: es
//! exactamente el estado del que venimos, donde daba 42 PASS sobre una
//! instalación que `nexum-verify-parity` y `nexum-registry-gate` rechazaban.
//!
//! Cada caso induce una discrepancia y verifica dos cosas: que el veredicto sea
//! Fail, y que el mensaje **nombre** el problema. Un FAIL sin nombre obliga a
//! diagnosticar de cero.

use super::*;

fn ids(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

// ─── catálogo ↔ registry ─────────────────────────────────────────────────────

#[test]
fn registry_catalog_pasa_cuando_coinciden() {
    let g = registry_catalog_gate(&ids(&["a", "b"]), &ids(&["a", "b"]));
    assert!(matches!(g, Gate::Pass(_)), "{g:?}");
}

#[test]
fn registry_catalog_falla_con_fila_de_mas_y_la_nombra() {
    // El defecto histórico: `opencode`, alias de opencode_zen con fila propia.
    let g = registry_catalog_gate(&ids(&["a", "b", "opencode"]), &ids(&["a", "b"]));
    let Gate::Fail(msg) = &g else {
        panic!("una fila de más TIENE que fallar, salió {g:?}");
    };
    assert!(msg.contains("opencode"), "el FAIL debe nombrar el provider: {msg}");
    assert!(msg.contains("NO en el registry"), "{msg}");
}

#[test]
fn registry_catalog_falla_con_fila_de_menos_y_la_nombra() {
    let g = registry_catalog_gate(&ids(&["a"]), &ids(&["a", "opencode_zen"]));
    let Gate::Fail(msg) = &g else {
        panic!("una fila de menos TIENE que fallar, salió {g:?}");
    };
    assert!(msg.contains("opencode_zen"), "{msg}");
    assert!(msg.contains("NO en el catálogo"), "{msg}");
}

// ─── estampa de generación ───────────────────────────────────────────────────

#[test]
fn generacion_pasa_cuando_coincide() {
    assert!(matches!(generation_gate(Some(4), 4), Gate::Pass(_)));
}

#[test]
fn generacion_falla_si_el_catalogo_es_posterior() {
    // El caso peligroso: puede conceder acceso con semántica desconocida.
    let g = generation_gate(Some(5), 4);
    let Gate::Fail(msg) = &g else {
        panic!("catálogo POSTERIOR tiene que fallar, salió {g:?}");
    };
    assert!(msg.contains("POSTERIOR"), "{msg}");
    assert!(msg.contains("catálogo=5") && msg.contains("binario=4"), "{msg}");
}

#[test]
fn generacion_falla_si_el_catalogo_es_anterior() {
    let g = generation_gate(Some(3), 4);
    let Gate::Fail(msg) = &g else {
        panic!("catálogo anterior tiene que fallar, salió {g:?}");
    };
    assert!(msg.contains("catálogo=3") && msg.contains("binario=4"), "{msg}");
    assert!(msg.contains("reconcile"), "el FAIL debe decir el remedio: {msg}");
}

#[test]
fn generacion_falla_sin_estampa() {
    // Catálogos escritos antes de 4.1. No rompen, pero no conceden.
    let g = generation_gate(None, 4);
    let Gate::Fail(msg) = &g else {
        panic!("sin estampa tiene que fallar, salió {g:?}");
    };
    assert!(msg.contains("no lleva estampa"), "{msg}");
}

// ─── integridad contra el manifiesto ─────────────────────────────────────────

#[test]
fn integridad_pasa_sin_discrepancias() {
    assert!(matches!(manifest_integrity_gate(100, &[]), Gate::Pass(_)));
}

#[test]
fn integridad_falla_y_nombra_los_archivos() {
    let g = manifest_integrity_gate(100, &["provider-catalog-base.json".into()]);
    let Gate::Fail(msg) = &g else {
        panic!("un hash que no coincide TIENE que fallar, salió {g:?}");
    };
    assert!(msg.contains("provider-catalog-base.json"), "{msg}");
}

#[test]
fn integridad_falla_sin_manifiesto_en_vez_de_pasar_vacia() {
    // Cero archivos revisados no es "todo bien": es que no se verificó nada.
    // Es el mismo error que un umbral disparando con cero muestras, al revés.
    let g = manifest_integrity_gate(0, &[]);
    let Gate::Fail(msg) = &g else {
        panic!("sin manifiesto NO puede pasar, salió {g:?}");
    };
    assert!(msg.contains("sin manifiesto"), "{msg}");
}

// ─── paridad contra el fuente ────────────────────────────────────────────────

#[test]
fn paridad_saltea_sin_referencia_y_dice_que_no_corrio() {
    let g = source_parity_gate(None, None);
    let Gate::Skip(msg) = &g else {
        panic!("sin referencia tiene que SALTEAR, no pasar: {g:?}");
    };
    assert!(
        msg.contains("NO se ejecutó"),
        "el SKIP tiene que decir que no corrió, si no es un PASS disfrazado: {msg}"
    );
    assert!(msg.contains("NEXUM_REF_DIR"), "{msg}");
}

#[test]
fn paridad_saltea_si_el_directorio_no_existe() {
    let g = source_parity_gate(Some(std::path::Path::new("/no/existe")), None);
    let Gate::Skip(msg) = &g else {
        panic!("referencia inexistente tiene que SALTEAR: {g:?}");
    };
    assert!(msg.contains("NO se ejecutó"), "{msg}");
}

#[test]
fn paridad_falla_con_diferencias_y_dice_como_verlas() {
    let g = source_parity_gate(Some(std::path::Path::new("/tmp/ref")), Some(6));
    let Gate::Fail(msg) = &g else {
        panic!("6 diferencias TIENEN que fallar, salió {g:?}");
    };
    assert!(msg.contains('6'), "{msg}");
    assert!(msg.contains("nexum-verify-parity"), "el FAIL debe dar el comando: {msg}");
}

#[test]
fn paridad_nunca_pasa_sin_haber_comparado() {
    // La propiedad central: un verificador que aprueba sin ejecutar es peor que
    // no tenerlo, porque da una respuesta y la respuesta está mal.
    for caso in [
        source_parity_gate(None, None),
        source_parity_gate(Some(std::path::Path::new("/no/existe")), None),
    ] {
        assert!(
            !matches!(caso, Gate::Pass(_)),
            "un gate que no se ejecutó no puede reportar Pass: {caso:?}"
        );
    }
}

// ─── procedencia ─────────────────────────────────────────────────────────────

const A: &str = "0088eae3cf48d9963820eaf5302ea4e4a98aafb4";
const B: &str = "094aea8396949350dd18f0c01c7152bed51a23dc";

#[test]
fn procedencia_falla_si_el_manifiesto_no_la_declara() {
    let g = provenance_gate(None, Some(A));
    let Gate::Fail(msg) = &g else {
        panic!("sin source_tree no se puede saber de dónde salió: {g:?}");
    };
    assert!(msg.contains("source_tree"), "{msg}");
}

/// El escenario que más veces mordió y que ningún otro check cubre por default:
/// el paquete se armó desde el árbol equivocado. Los hashes internos son
/// consistentes entre sí — solo que del árbol de al lado.
#[test]
fn procedencia_falla_si_el_arbol_no_es_el_esperado() {
    let g = provenance_gate(Some(A), Some(B));
    let Gate::Fail(msg) = &g else {
        panic!("dos árboles distintos TIENEN que fallar: {g:?}");
    };
    assert!(msg.contains("OTRO ÁRBOL"), "{msg}");
    assert!(msg.contains(A) && msg.contains(B), "el FAIL debe nombrar los dos: {msg}");
}

#[test]
fn procedencia_pasa_cuando_coinciden() {
    assert!(matches!(provenance_gate(Some(A), Some(A)), Gate::Pass(_)));
}

/// Sin referencia local no se puede contrastar, pero el paquete SÍ declara su
/// árbol: eso es información real, no una verificación omitida. Por eso es Pass
/// y no Skip — y el mensaje dice qué falta para contrastarlo.
#[test]
fn procedencia_sin_referencia_reporta_el_arbol_declarado() {
    let g = provenance_gate(Some(A), None);
    let Gate::Pass(msg) = &g else { panic!("{g:?}") };
    assert!(msg.contains(A), "{msg}");
    assert!(msg.contains("NEXUM_REF_DIR"), "{msg}");
}
