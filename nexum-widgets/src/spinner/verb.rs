use rand::RngExt;

/// Nexum — Verbos aleatorios en español rioplatense para el spinner de carga.
pub const DEFAULT_VERBS: &[&str] = &[
    // ── Pensamiento / Análisis ──
    "pensando",
    "analizando",
    "calculando",
    "razonando",
    "examinando",
    "evaluando",
    "reflexionando",
    "considerando",
    "procesando",
    "interpretando",
    // ── Escritura / Creación ──
    "escribiendo",
    "creando",
    "diseñando",
    "armando",
    "componiendo",
    "redactando",
    "generando",
    "maquetando",
    "estructurando",
    "fabricando",
    // ── Búsqueda / Lectura ──
    "buscando",
    "leyendo",
    "escaneando",
    "recorriendo",
    "rastreando",
    "revisando",
    "inspeccionando",
    "consultando",
    "extrayendo",
    "navegando",
    // ── Compilación / Transformación ──
    "compilando",
    "transformando",
    "traduciendo",
    "parseando",
    "resolviendo",
    "optimizando",
    "refactorizando",
    "limpiando",
    "desplegando",
    "integrando",
    // ── Acción / Ejecución ──
    "ejecutando",
    "corriendo",
    "aplicando",
    "sincronizando",
    "cargando",
    "descargando",
    "conectando",
    "transfiriendo",
    "iniciando",
    "finalizando",
    // ── Coloquiales argentinos ──
    "chamuyando",
    "cocinando",
    "tirando código",
    "dale que va",
    "rompiendo todo",
    "arreglando",
    "boludeando",
    "debuggeando",
    "zarpando",
    "meta laburar",
    // ── Conceptos / Abstracto ──
    "fusionando",
    "reorganizando",
    "clasificando",
    "mapeando",
    "indexando",
    "ordenando",
    "filtrando",
    "agregando",
    "combinando",
    "dividiendo",
    // ── Otros ──
    "trabajando",
    "ensamblando",
    "recolectando",
    "explorando",
    "monitoreando",
    "verificando",
    "probando",
    "validando",
    "ajustando",
    "mejorando",
];

pub fn pick_verb(active_form: Option<&str>) -> String {
    active_form.map(|s| format!("{}…", s)).unwrap_or_else(|| {
        let mut rng = rand::rng();
        DEFAULT_VERBS[rng.random_range(0..DEFAULT_VERBS.len())].to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_verb_with_active_form() {
        let result = pick_verb(Some("搜索文件"));
        assert!(
            result.contains("搜索文件…"),
            "expected '搜索文件…', got '{}'",
            result
        );
    }

    #[test]
    fn test_pick_verb_random() {
        let result = pick_verb(None);
        assert!(!result.is_empty(), "verb should not be empty");
        assert!(
            DEFAULT_VERBS.contains(&result.as_str()),
            "'{}' should be in DEFAULT_VERBS",
            result
        );
    }
}
