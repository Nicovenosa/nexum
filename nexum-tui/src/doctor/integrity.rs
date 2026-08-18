//! Los gates de empaquetado, dentro de Doctor.
//!
//! Doctor podía dar 42 PASS sobre una instalación que `nexum-verify-parity` y
//! `nexum-registry-gate` rechazaban. Un verificador que aprueba una instalación
//! rota es peor que no tener verificador: da una respuesta, y la respuesta está
//! mal.
//!
//! # Barato por diseño
//!
//! Doctor tiene que seguir siendo barato o la gente deja de correrlo y se pierde
//! el chequeo. Los tres de acá leen archivos que ya están en el slot y no tocan
//! la red. El único caro —paridad contra el árbol fuente— necesita un checkout
//! que en una máquina de usuario no existe, así que queda detrás de
//! `NEXUM_REF_DIR` y **se reporta como SKIP diciendo que no corrió**. Nunca PASS
//! sin haberse ejecutado: eso sería el mismo problema con otra cara.

use std::collections::BTreeSet;
use std::path::Path;

use super::{CheckResult, Status};

/// Resultado de un gate: el mensaje viaja con el veredicto para que el motivo
/// no se pierda entre la evaluación y el render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Gate {
    Pass(String),
    Fail(String),
    Skip(String),
}

impl Gate {
    fn status(&self) -> Status {
        match self {
            Gate::Pass(_) => Status::Pass,
            Gate::Fail(_) => Status::Fail,
            Gate::Skip(_) => Status::Skip,
        }
    }
    fn evidence(&self) -> &str {
        match self {
            Gate::Pass(m) | Gate::Fail(m) | Gate::Skip(m) => m,
        }
    }
}

/// Catálogo y registry tienen que declarar los MISMOS providers.
///
/// Un provider en un lado y no en el otro produce una fila fantasma —visible en
/// el panel, sin ruta de ejecución— o un provider invisible. Es el defecto que
/// costó siete bugs.
pub(crate) fn registry_catalog_gate(catalog: &BTreeSet<String>, registry: &BTreeSet<String>) -> Gate {
    let solo_catalogo: Vec<&str> = catalog.difference(registry).map(String::as_str).collect();
    let solo_registry: Vec<&str> = registry.difference(catalog).map(String::as_str).collect();
    if solo_catalogo.is_empty() && solo_registry.is_empty() {
        return Gate::Pass(format!(
            "catálogo y registry declaran los mismos {} providers",
            catalog.len()
        ));
    }
    let mut detalle = Vec::new();
    if !solo_catalogo.is_empty() {
        detalle.push(format!(
            "en el catálogo y NO en el registry: {}",
            solo_catalogo.join(", ")
        ));
    }
    if !solo_registry.is_empty() {
        detalle.push(format!(
            "en el registry y NO en el catálogo: {}",
            solo_registry.join(", ")
        ));
    }
    Gate::Fail(format!(
        "{} · catálogo={} registry={}",
        detalle.join(" · "),
        catalog.len(),
        registry.len()
    ))
}

/// La estampa: ¿el catálogo fue escrito por una generación que este binario
/// entiende?
///
/// Asimétrica a propósito, y **forward-only por construcción**: un binario
/// anterior a la guarda no la lleva, así que el cruce inverso —binario viejo con
/// catálogo nuevo, que es el rollback— no lo detecta nadie. Ver
/// docs/RUNBOOK-ROLLBACK.md.
pub(crate) fn generation_gate(catalog: Option<u64>, binario: u64) -> Gate {
    match catalog {
        None => Gate::Fail(format!(
            "el catálogo no lleva estampa de generación (binario={binario}). \
             Efecto: los providers de acceso libre quedan deshabilitados. \
             Remedio: `nexum provider reconcile`"
        )),
        Some(g) if g == binario => Gate::Pass(format!("generación {g}, coincide con el binario")),
        Some(g) if g > binario => Gate::Fail(format!(
            "catálogo de generación POSTERIOR (catálogo={g}, binario={binario}): \
             puede conceder acceso con una semántica que este binario no conoce. \
             Remedio: actualizá Nexum, o `nexum provider reconcile` con este binario"
        )),
        Some(g) => Gate::Fail(format!(
            "catálogo de generación anterior (catálogo={g}, binario={binario}). \
             Remedio: `nexum provider reconcile`"
        )),
    }
}

/// Integridad del paquete instalado contra los hashes que él mismo declara.
///
/// OJO CON EL ALCANCE: esto detecta que un archivo cambió DESPUÉS de empaquetar
/// —corrupción, edición a mano, copia incompleta— y **no** detecta que el
/// paquete se armó desde el árbol equivocado. Para eso hace falta comparar
/// contra el fuente, que es el gate de abajo. Confundir los dos sería la misma
/// tautología que apuntar `nexum-verify-parity` al `HASHES.tsv` del propio slot.
pub(crate) fn manifest_integrity_gate(revisados: usize, discrepantes: &[String]) -> Gate {
    if revisados == 0 {
        return Gate::Fail(
            "no se pudo leer PACKAGE_MANIFEST.json: sin manifiesto no hay nada que verificar"
                .to_string(),
        );
    }
    if discrepantes.is_empty() {
        return Gate::Pass(format!(
            "{revisados} archivos coinciden con el hash que declara el manifiesto"
        ));
    }
    Gate::Fail(format!(
        "{} de {revisados} archivos no coinciden con su hash declarado: {}",
        discrepantes.len(),
        discrepantes.join(", ")
    ))
}

/// Paridad contra el árbol fuente. El caro: necesita un checkout.
///
/// Si no hay referencia, SKIP con el motivo. Nunca PASS: decir "todo bien" sin
/// haber comparado es exactamente lo que hace que un verificador sea peor que
/// ninguno.
pub(crate) fn source_parity_gate(referencia: Option<&Path>, diferencias: Option<usize>) -> Gate {
    let Some(dir) = referencia else {
        return Gate::Skip(
            "sin árbol fuente de referencia: seteá NEXUM_REF_DIR para compararlo. \
             NO se ejecutó — esto no es un PASS"
                .to_string(),
        );
    };
    match diferencias {
        None => Gate::Skip(format!(
            "el directorio de referencia no existe: {}. NO se ejecutó",
            dir.display()
        )),
        Some(0) => Gate::Pass(format!("sin diferencias contra {}", dir.display())),
        Some(n) => Gate::Fail(format!(
            "{n} diferencias contra el árbol fuente {}. \
             Detalle: `NEXUM_REF_DIR={} nexum-verify-parity`",
            dir.display(),
            dir.display()
        )),
    }
}

/// ¿El paquete declara de qué árbol salió, y ese árbol es el que se espera?
///
/// Cierra el escenario que más veces nos mordió y que ningún check cubría por
/// defecto: **armado desde el árbol equivocado**. `PKG-MANIFEST-INTEGRITY` no lo
/// ve —los hashes serían consistentes entre sí, solo que del árbol de al lado— y
/// `PKG-SOURCE-PARITY` sí, pero está detrás de un flag y por default no corre.
///
/// Es barato porque compara UN sha contra otro, sin recorrer árboles:
/// `nexum-package` ya exige `NEXUM_SOURCE_TREE` y lo estampa en el manifiesto.
pub(crate) fn provenance_gate(estampado: Option<&str>, esperado: Option<&str>) -> Gate {
    let Some(sellado) = estampado else {
        return Gate::Fail(
            "el manifiesto no declara source_tree: no se puede saber de qué árbol salió              este paquete. Reempaquetá con nexum-package, que lo exige"
                .to_string(),
        );
    };
    match esperado {
        None => Gate::Pass(format!(
            "paquete armado desde el árbol {sellado} (sin referencia local para contrastar;              seteá NEXUM_REF_DIR para verificarlo)"
        )),
        Some(e) if e == sellado => {
            Gate::Pass(format!("árbol {sellado}, coincide con la referencia"))
        }
        Some(e) => Gate::Fail(format!(
            "ARMADO DESDE OTRO ÁRBOL: el paquete declara {sellado} y la referencia está en {e} · los hashes internos pueden ser consistentes entre sí y aun así ser el árbol equivocado"
        )),
    }
}

fn push(out: &mut Vec<CheckResult>, id: &'static str, desc: &str, gate: Gate) {
    out.push(CheckResult::new(
        id,
        gate.status(),
        desc,
        gate.evidence(),
    ));
}

/// Los gates de empaquetado, cableados al disco.
pub fn integrity(out: &mut Vec<CheckResult>) {
    // Doctor corre SIN la TUI, así que es una de las entradas que puede rescatar
    // las tareas de cron antes de que el logout borre el runtime dir. Es
    // idempotente: si ya migró, no hace nada.
    nexum_agent::config_home::migrate_cron_store_if_needed();

    let layout = crate::layout::InstalledLayoutV1::current();

    // ── catálogo ↔ registry ──────────────────────────────────────────────────
    let gate = match (catalog_provider_ids(), registry_provider_ids()) {
        (Some(c), Some(r)) => registry_catalog_gate(&c, &r),
        _ => Gate::Fail(
            "no se pudieron leer el catálogo y el route registry instalados".to_string(),
        ),
    };
    push(
        out,
        "PKG-REGISTRY-CATALOG",
        "catálogo y route registry declaran los mismos providers",
        gate,
    );

    // ── estampa de generación ────────────────────────────────────────────────
    let catalog_gen = nexum_acp::provider::load_live_catalog()
        .and_then(|doc| doc.get("catalog_generation").and_then(|v| v.as_u64()));
    push(
        out,
        "PKG-CATALOG-GENERATION",
        "la estampa del catálogo coincide con la del binario",
        generation_gate(catalog_gen, nexum_acp::provider::CATALOG_GENERATION),
    );

    // ── integridad contra el manifiesto ──────────────────────────────────────
    let (revisados, discrepantes) = layout
        .as_ref()
        .map(|l| verificar_manifiesto(&l.version_root()))
        .unwrap_or((0, Vec::new()));
    push(
        out,
        "PKG-MANIFEST-INTEGRITY",
        "los archivos instalados coinciden con los hashes del manifiesto",
        manifest_integrity_gate(revisados, &discrepantes),
    );

    // ── procedencia: barato, un sha contra otro ──────────────────────────────
    let estampado = layout.as_ref().and_then(|l| manifest_source_tree(&l.version_root()));
    let referencia_dir = std::env::var_os("NEXUM_REF_DIR").map(std::path::PathBuf::from);
    let esperado = referencia_dir.as_deref().and_then(git_tree_of);
    push(
        out,
        "PKG-SOURCE-PROVENANCE",
        "el paquete declara de qué árbol se armó",
        provenance_gate(estampado.as_deref(), esperado.as_deref()),
    );

    // ── paridad contra el fuente (caro: detrás de NEXUM_REF_DIR) ─────────────
    let referencia = std::env::var_os("NEXUM_REF_DIR").map(std::path::PathBuf::from);
    let gate = match referencia.as_deref() {
        None => source_parity_gate(None, None),
        Some(dir) if !dir.is_dir() => source_parity_gate(Some(dir), None),
        Some(dir) => {
            let n = layout
                .as_ref()
                .map(|l| contar_diferencias(dir, &l.version_root()))
                .unwrap_or(0);
            source_parity_gate(Some(dir), Some(n))
        }
    };
    push(
        out,
        "PKG-SOURCE-PARITY",
        "el paquete instalado refleja el árbol fuente",
        gate,
    );
}

/// `source_tree` del manifiesto instalado.
fn manifest_source_tree(root: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(root.join("MANIFEST.json")).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&raw).ok()?;
    doc.get("source_tree")
        .and_then(|v| v.as_str())
        .filter(|s| s.len() == 40)
        .map(str::to_string)
}

/// Árbol del HEAD de un checkout. Un solo `git rev-parse`, sin recorrer nada.
fn git_tree_of(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD^{tree}"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (s.len() == 40).then_some(s)
}

fn catalog_provider_ids() -> Option<BTreeSet<String>> {
    let (doc, _) = crate::app::provider_panel::load_catalog_document().ok()?;
    Some(
        doc.providers
            .iter()
            .map(|p| p.stable_id().replace('-', "_"))
            .collect(),
    )
}

/// Ids del route registry, SIN validarlo contra el catálogo.
///
/// Usaba `validate_installed_registry`, que valida catálogo↔registry por su
/// cuenta y devuelve Err ante la primera discrepancia — justo el caso que este
/// gate existe para reportar. El resultado era un FAIL genérico ("no se pudieron
/// leer") en vez del nombre del provider, y un FAIL sin nombre obliga a
/// diagnosticar de cero. La comparación la hace el gate; acá sólo se lee.
fn registry_provider_ids() -> Option<BTreeSet<String>> {
    let (registry, _) = nexum_acp::provider::routes::ProviderRouteRegistry::load_installed().ok()?;
    Some(
        registry
            .routes
            .iter()
            .map(|r| r.provider_id.replace('-', "_"))
            .collect(),
    )
}

fn sha256_file(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", Sha256::digest(&bytes)))
}

/// Compara cada archivo declarado en PACKAGE_MANIFEST.json contra su hash.
fn verificar_manifiesto(root: &Path) -> (usize, Vec<String>) {
    let Ok(raw) = std::fs::read_to_string(root.join("PACKAGE_MANIFEST.json")) else {
        return (0, Vec::new());
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (0, Vec::new());
    };
    let Some(files) = doc.get("files").and_then(|f| f.as_array()) else {
        return (0, Vec::new());
    };
    let mut revisados = 0usize;
    let mut malos = Vec::new();
    for f in files {
        let (Some(rel), Some(esperado)) = (
            f.get("path").and_then(|v| v.as_str()),
            f.get("sha256").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let path = root.join(rel);
        if !path.is_file() {
            malos.push(format!("{rel} (ausente)"));
            revisados += 1;
            continue;
        }
        revisados += 1;
        if sha256_file(&path).as_deref() != Some(esperado) {
            malos.push(rel.to_string());
        }
        if malos.len() >= 5 {
            malos.push("…".to_string());
            break;
        }
    }
    (revisados, malos)
}

/// Cuenta archivos del paquete cuyo hash difiere del homónimo en el fuente.
fn contar_diferencias(referencia: &Path, root: &Path) -> usize {
    let mut n = 0usize;
    for rel in ["provider-catalog-base.json", "catalog-contract.json", "NOTICE"] {
        let a = referencia.join("config").join(rel);
        let a = if a.is_file() { a } else { referencia.join(rel) };
        let b = root.join(rel);
        if a.is_file() && b.is_file() && sha256_file(&a) != sha256_file(&b) {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
#[path = "integrity_test.rs"]
mod tests;
