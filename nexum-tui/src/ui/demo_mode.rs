//! Public demo mode (sprint PUBLIC DEMO 2026-07-07).
//!
//! `NEXUM_PUBLIC_DEMO=1` es un master switch de seguridad/limpieza visual
//! para grabar demos públicas (LinkedIn, capturas, video). Cuando está
//! activo, FUERZA los flags de UX que ya son default-off en Nexum, sin
//! importar overrides locales del operador — así una demo grabada con este
//! flag nunca puede quedar contaminada por config previa de la máquina
//! (ej. alguien había dejado `NEXUM_PREDICTIONS=on` en su shell rc).
//!
//! No reemplaza la redacción de secretos (`ui::secret_redact`, que corre
//! SIEMPRE, esté o no en demo mode) ni el masking de API keys en /provedor
//! (que ya no muestra keys crudas por diseño). Es una capa adicional de
//! "modo presentación": nada de ghost text, nada de barras duplicadas,
//! nada de logs de debug con paths internos.

/// ¿Está activo el modo demo pública?
pub fn public_demo_enabled() -> bool {
    matches!(
        std::env::var("NEXUM_PUBLIC_DEMO")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "on" | "1" | "true" | "yes"
    )
}

/// Lock GLOBAL para tests que mutan env vars de flags (NEXUM_PUBLIC_DEMO,
/// NEXUM_PREDICTIONS, NEXUM_HORMIGUERO, ...). El env es global al proceso y
/// los tests corren en paralelo: un guard local por módulo NO serializa
/// contra los demás módulos (carrera observada entre demo_mode/composer/
/// hormiguero). Todo test que haga set_var/remove_var de flags debe tomar
/// ESTE lock.
#[cfg(test)]
pub fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::public_demo_enabled;

    fn with_env<T>(val: Option<&str>, f: impl FnOnce() -> T) -> T {
        let prev = std::env::var("NEXUM_PUBLIC_DEMO").ok();
        match val {
            Some(v) => std::env::set_var("NEXUM_PUBLIC_DEMO", v),
            None => std::env::remove_var("NEXUM_PUBLIC_DEMO"),
        }
        let out = f();
        match prev {
            Some(v) => std::env::set_var("NEXUM_PUBLIC_DEMO", v),
            None => std::env::remove_var("NEXUM_PUBLIC_DEMO"),
        }
        out
    }

    #[test]
    fn test_public_demo_flag_matrix() {
        let _g = super::test_env_lock();
        assert!(!with_env(None, public_demo_enabled), "default off");
        for v in ["on", "1", "true", "yes", "ON"] {
            assert!(with_env(Some(v), public_demo_enabled), "{v} habilita");
        }
        for v in ["off", "0", "false", "", "no"] {
            assert!(!with_env(Some(v), public_demo_enabled), "{v} NO habilita");
        }
    }
}
