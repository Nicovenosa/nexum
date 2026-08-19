//! Detección de entorno de test, compartida por todos los crates.
//!
//! # Por qué existe un solo lugar
//!
//! El proyecto tenía cuatro mecanismos de aislamiento con la misma forma
//! —`CatalogoAislado`, `NEXUM_CATALOG_ISOLATED`, el temporal por PID de
//! `turn_log::metrics_dir`, y ahora el runtime dir— escritos por separado. Y el
//! patrón correcto convivía con el incorrecto en el mismo repo: cuatro archivos
//! usaban PID y siete un nombre fijo.
//!
//! Eso no es casualidad ni descuido: **nunca hubo un solo lugar del que
//! colgar**, así que cada vez que hizo falta aislar algo se escribió de nuevo, y
//! escribir la versión mala siempre costó menos que la buena. Este módulo es ese
//! lugar. Para que sirva tiene que ser más fácil de usar que la alternativa —
//! por eso el lint prohíbe `std::env::temp_dir()` fuera de acá.
//!
//! # Por qué la detección es en runtime
//!
//! `cfg!(test)` no alcanza: cuando `nexum-tui` corre sus tests, este crate se
//! compila como dependencia normal y su `cfg(test)` está apagado. La señal tiene
//! que cruzar el límite de crate, y la única disponible en runtime es la ruta
//! del ejecutable — cargo compila los tests a `<target>/<profile>/deps/`.
//!
//! # Fail-closed
//!
//! El aislamiento NO depende de que cada test se acuerde de optar. Ya falló una
//! vez así: la deuda estaba documentada tres turnos antes de que una corrida de
//! la suite contaminara la evidencia de un experimento. Si el ejecutable es un
//! binario de test, se aísla aunque nadie haya seteado nada.

use std::path::PathBuf;

/// ¿El ejecutable actual es un binario de test de cargo?
pub fn running_under_test() -> bool {
    std::env::current_exe()
        .ok()
        .map(|p| {
            // Normalizar separadores: en Windows el path usa '\' y los
            // marcadores de binario de test no coincidirían con '/'.
            let s = p
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            s.contains("/deps/")
                || s.contains("/target/debug/")
                || s.contains("/target/release/deps/")
        })
        .unwrap_or(false)
}

/// Sufijo único por proceso, para recursos de SESIÓN.
///
/// Sólo para dato que muere con el proceso: sockets, materializaciones
/// efímeras, métricas de test. **Nunca para dato del usuario** — ya casi
/// costó el historial de conversaciones y las tareas de cron, que por PID
/// habrían quedado vacíos en cada arranque. La pregunta no es "¿colisiona?"
/// sino "¿qué vida tiene este dato?".
pub fn session_suffix() -> String {
    format!("test-{}", std::process::id())
}

/// Directorio temporal aislado por proceso.
///
/// El reemplazo de `std::env::temp_dir().join("<nombre-fijo>")`, que es lo que
/// produjo dos bugs de producción: `/tmp/zen-threads.db` compartido entre
/// instancias y `/tmp/nexum-settings-override.json` pisándose entre
/// invocaciones.
#[allow(clippy::disallowed_methods)] // el sandbox ES el lugar autorizado a llamar temp_dir()
pub fn temp_dir_for(nombre: &str) -> PathBuf {
    // El nombre DICE que es aislamiento de test: `nexum-<qué>-test-<pid>`. Si
    // alguien encuentra uno de estos en /tmp tiene que poder saber de dónde
    // salió sin leer código.
    std::env::temp_dir().join(format!("nexum-{nombre}-{}", session_suffix()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Este test corre DENTRO de un binario de test: si la detección se rompe,
    /// falla nombrando la regresión en vez de pasar en silencio.
    #[test]
    fn la_deteccion_se_reconoce_a_si_misma() {
        assert!(
            running_under_test(),
            "un test que no se detecta como test deja el aislamiento apagado \
             justo donde hace falta"
        );
    }

    #[test]
    fn el_temporal_lleva_el_pid() {
        let d = temp_dir_for("cosa");
        assert!(d.to_string_lossy().contains(&std::process::id().to_string()));
    }
}
