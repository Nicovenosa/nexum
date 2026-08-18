use super::*;

/// `XDG_DATA_HOME` es estado global del proceso; los tests que lo tocan se
/// serializan entre sí.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn con_xdg<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let previo = std::env::var("XDG_DATA_HOME").ok();
    std::env::set_var("XDG_DATA_HOME", dir);
    let out = f();
    match previo {
        Some(v) => std::env::set_var("XDG_DATA_HOME", v),
        None => std::env::remove_var("XDG_DATA_HOME"),
    }
    out
}

fn escribir(path: &Path, contenido: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contenido).unwrap();
}

#[test]
fn el_vivo_gana_cuando_es_valido() {
    let tmp = tempfile::tempdir().unwrap();
    con_xdg(tmp.path(), || {
        escribir(&live_catalog().unwrap(), r#"{"providers":[]}"#);
        let r = resolve();
        assert_eq!(r.source, CatalogSource::Live);
        assert_eq!(r.path, live_catalog().unwrap());
        assert!(!r.live_rejected);
    });
}

#[test]
fn un_vivo_corrupto_cae_al_snapshot_previo_y_lo_dice() {
    let tmp = tempfile::tempdir().unwrap();
    con_xdg(tmp.path(), || {
        escribir(&live_catalog().unwrap(), "{ roto");
        escribir(&previous_catalog().unwrap(), r#"{"providers":[]}"#);
        let r = resolve();
        assert_eq!(r.source, CatalogSource::Previous);
        assert!(
            r.live_rejected,
            "un vivo roto no puede reportarse como ausente"
        );
    });
}

/// El defecto que motivó este módulo: el panel leía el catálogo vivo y el
/// resolvedor de rutas de ejecución leía la copia congelada del slot. El panel
/// mostraba un provider usable y el turno salía a otro endpoint.
#[test]
fn produccion_y_ejecucion_resuelven_el_mismo_archivo() {
    let tmp = tempfile::tempdir().unwrap();
    con_xdg(tmp.path(), || {
        escribir(&live_catalog().unwrap(), r#"{"providers":[]}"#);
        let panel = resolve().path;
        let ejecucion = resolve().path;
        assert_eq!(
            panel, ejecucion,
            "si estas dos divergen vuelve el 502: el panel dice usable y el turno va a otro lado"
        );
    });
}

#[test]
fn el_directorio_xdg_es_el_que_reconcile_escribe() {
    let tmp = tempfile::tempdir().unwrap();
    con_xdg(tmp.path(), || {
        assert_eq!(
            providers_dir().unwrap(),
            tmp.path().join("nexum/providers"),
            "reconcile publica acá; un fixture que escriba otra ruta no lo ve nadie"
        );
    });
}

/// La trampa que este flag cierra: apuntar `XDG_DATA_HOME` a un temp vacío no
/// deja al resolver sin catálogo. Sin la guarda seguía hasta la base del
/// checkout —que existe siempre que haya repo— y devolvía providers reales de
/// la máquina. Un test que se cree aislado leyendo el estado del desarrollador
/// es la misma trampa que el path de /tmp, una capa más arriba.
#[test]
fn el_aislamiento_neutraliza_la_cadena_entera_no_solo_el_vivo() {
    let tmp = tempfile::tempdir().unwrap();
    con_xdg(tmp.path(), || {
        let sin_guarda = resolve();
        assert_ne!(
            sin_guarda.source,
            CatalogSource::Missing,
            "precondición: sin la guarda el resolver SÍ encuentra un catálogo real"
        );

        std::env::set_var(ISOLATED_ENV, "1");
        let con_guarda = resolve();
        std::env::remove_var(ISOLATED_ENV);

        assert_eq!(
            con_guarda.source,
            CatalogSource::Missing,
            "bajo aislamiento no puede haber catálogo: salió de {:?}",
            con_guarda.path
        );
    });
}

#[test]
fn bajo_aislamiento_el_vivo_del_temp_sigue_valiendo() {
    let tmp = tempfile::tempdir().unwrap();
    con_xdg(tmp.path(), || {
        std::env::set_var(ISOLATED_ENV, "1");
        escribir(&live_catalog().unwrap(), r#"{"providers":[]}"#);
        let r = resolve();
        std::env::remove_var(ISOLATED_ENV);
        assert_eq!(
            r.source,
            CatalogSource::Live,
            "el aislamiento apaga los fallbacks, no el catálogo que el fixture escribe"
        );
    });
}
