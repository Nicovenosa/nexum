//! Fixture de catálogo para los tests que leen `provider-catalog-live.json`.
//!
//! Diez tests de `model_panel` y del panel de modelos se llamaban a sí mismos
//! `..._without_catalog` pero no aislaban nada: leían
//! `$XDG_DATA_HOME/nexum/providers/provider-catalog-live.json` de la máquina y
//! asumían que no existía. En una instalación real existe, así que fallaban con
//! `left: 56, right: 0` — y el conteo de fallos de la suite pasaba a depender de
//! cuántos providers tuviera conectados quien la corriera. Un baseline que se
//! mueve con el entorno no sirve como baseline.
//!
//! [`CatalogoAislado`] apunta `XDG_DATA_HOME` a un directorio propio mientras
//! vive y lo restaura al soltarse.
//!
//! **El test debe tomar `demo_mode::test_env_lock()` primero.** `XDG_DATA_HOME`
//! es estado del proceso: sin candado, dos tests en paralelo se leen el
//! directorio del otro, y `doctor/doctor_test.rs` también lo escribe.
//!
//! El guard NO toma el candado por su cuenta, a propósito: varios de los tests
//! que lo usan ya lo tenían tomado, y un `Mutex` de `std` no es reentrante —
//! tomarlo acá los colgaba. Un solo candado, tomado en un solo lugar.

#![cfg(test)]

/// Guard RAII: mientras vive, `XDG_DATA_HOME` apunta a un directorio temporal
/// bajo control del test.
pub(crate) struct CatalogoAislado {
    dir: std::path::PathBuf,
    previo: Option<String>,
}

impl CatalogoAislado {
    /// Sin catálogo: el directorio existe pero está vacío.
    ///
    /// Es lo que los tests `..._without_catalog` creían tener.
    pub(crate) fn vacio() -> Self {
        Self::nuevo(None)
    }

    /// Con un catálogo controlado por el test.
    #[allow(dead_code)]
    /// Completa el piso que el contrato exige.
    ///
    /// `validate_at` rechaza un catálogo al que le falte cualquiera de los
    /// `required_providers_with_models`. Un fixture mínimo no valida, y si cada
    /// test los repite a mano vuelven a ser N copias de la misma verdad. El
    /// piso se deriva del contrato; el test declara solo lo que le importa y
    /// eso pisa al stub.
    fn con_piso_del_contrato(mut doc: serde_json::Value) -> serde_json::Value {
        const CONTRATO: &str = include_str!("../../../config/catalog-contract.json");
        let contrato: serde_json::Value =
            serde_json::from_str(CONTRATO).expect("el contrato tiene que parsear");
        let requeridos: Vec<String> = contrato["required_providers_with_models"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        if doc.get("schema_version").is_none() {
            doc["schema_version"] = contrato["catalog_schema_version"].clone();
        }
        let declarados: std::collections::HashSet<String> = doc["providers"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.get("id").and_then(|v| v.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let providers = doc["providers"]
            .as_array_mut()
            .expect("el fixture declara `providers`");
        for req in requeridos {
            if declarados.contains(&req) {
                continue;
            }
            providers.push(serde_json::json!({
                "id": req,
                "display_name": req,
                "usable_now": false,
                "models": [format!("{req}-stub")],
            }));
        }
        doc
    }

    pub(crate) fn con(doc: serde_json::Value) -> Self {
        let doc = Self::con_piso_del_contrato(doc);
        Self::nuevo(Some(doc))
    }

    fn nuevo(doc: Option<serde_json::Value>) -> Self {
        // ALLOW justificado: lleva PID y contador nano (líneas de abajo); es el
        // fixture que AÍSLA el catálogo, así que no puede colgar de sí mismo.
        #[allow(clippy::disallowed_methods)]
        let dir = std::env::temp_dir().join(format!(
            "nexum-catalogo-fixture-{}-{}",
            std::process::id(),
            // Un directorio distinto por instancia evita que un remanente de
            // una corrida anterior se filtre.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // La ruta NO se construye acá. Un fixture que arma su propio path deja
        // de probar lo que el producto lee en cuanto alguien mueve el archivo:
        // es la segunda copia de la misma verdad, que es el defecto que este
        // resolvedor vino a cerrar. Se apunta XDG primero y se pregunta.
        let previo = std::env::var("XDG_DATA_HOME").ok();
        // SAFETY: el candado de entorno que tomó el test garantiza que ningún
        // otro test está leyendo o escribiendo `XDG_DATA_HOME` mientras tanto.
        unsafe { std::env::set_var("XDG_DATA_HOME", &dir) };
        // Apuntar XDG NO aísla: el resolver sigue bajando la cadena hasta la
        // base instalada o la del checkout, las dos reales, y el test termina
        // leyendo el estado de la máquina creyendo que no hay catálogo.
        // SAFETY: mismo candado de entorno.
        unsafe {
            std::env::set_var(nexum_acp::provider::catalog_path::ISOLATED_ENV, "1")
        };
        let destino = nexum_acp::provider::catalog_path::live_catalog()
            .expect("catalog_path debe resolver con XDG_DATA_HOME seteado");
        std::fs::create_dir_all(destino.parent().expect("el catálogo vive en un directorio"))
            .expect("crear el directorio de la fixture");
        if let Some(doc) = doc {
            std::fs::write(
                &destino,
                serde_json::to_string(&doc).expect("serializar el catálogo de la fixture"),
            )
            .expect("escribir el catálogo de la fixture");
        }

        Self { dir, previo }
    }
}

impl Drop for CatalogoAislado {
    fn drop(&mut self) {
        // SAFETY: el candado que tomó el test sigue vivo.
        unsafe { std::env::remove_var(nexum_acp::provider::catalog_path::ISOLATED_ENV) };
        // SAFETY: el candado que tomó el test sigue vivo — este guard se
        // declara después, así que se suelta antes.
        unsafe {
            match self.previo.take() {
                Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
