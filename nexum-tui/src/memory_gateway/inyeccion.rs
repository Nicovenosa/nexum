//! Selección de qué memoria entra al prompt, con presupuesto y umbral.
//!
//! Separado del cliente HTTP a propósito: acá no hay red ni I/O, así que la
//! decisión —la parte que puede arruinar una respuesta— se prueba sola.
//!
//! # Por qué el umbral es RELATIVO y no un número fijo
//!
//! Medido en M1 (Vivobook Go E1504FA) el 2026-08-03, con el mismo store y dos
//! tamaños de corpus:
//!
//! ```text
//! 8 memorias    q="Nico"                        0.60 0.58 0.54   (max 3.85)
//! 200 memorias  q="runtime presupuesto..."      p50 2.04  min 1.90 (max 4.19)
//! 200 memorias  q="token sesion aislamiento"    p50 5.46  min 2.73 (max 5.88)
//! ```
//!
//! Un umbral absoluto calibrado con 8 memorias —1.0 parecía razonable— **deja
//! pasar casi todo con 200**, porque bm25 no produce una escala estable: depende
//! del tamaño del corpus y de qué tan raros sean los términos de la consulta.
//!
//! Por eso el corte es una **fracción del mejor resultado de esa consulta**. Se
//! adapta solo al corpus y a la especificidad: si el mejor match es flojo, todo
//! lo demás también lo es y no entra nada.
//!
//! # Por qué hay además un techo de tokens
//!
//! El umbral controla la CALIDAD, no la CANTIDAD. Con 200 memorias una consulta
//! genérica devolvió 50 resultados y **961 tokens** —contra 116 con 8 memorias,
//! ocho veces más—. Sin techo, la memoria compite con la conversación por el
//! contexto y empeora las respuestas que venía a mejorar.
//!
//! El recorte saca **entradas enteras, de menor a mayor relevancia**. Nunca corta
//! una entrada por el medio: media memoria puede decir lo contrario que la
//! entera.

/// Fracción del mejor score por debajo de la cual una entrada no entra.
///
/// Default 0,60: verificado contra los dos corpus de arriba. Con
/// `q="flag por defecto"` (genérica, max 4,66) deja fuera la mediana; con
/// `q="token sesion aislamiento"` (específica, max 5,88) deja pasar casi todo,
/// que es lo correcto porque ahí todos los resultados SON relevantes.
///
/// Configurable por `NEXUM_MEMORY_UMBRAL` — el valor de acá salió de un corpus
/// sintético en una máquina, y con otro perfil de uso puede no servir.
pub const UMBRAL_RELATIVO_DEFAULT: f32 = 0.60;

/// Techo de tokens que la memoria puede agregar al prompt.
///
/// Default 400: el peor caso medido fue 961 tokens sin recorte, y 400 deja el
/// resto del contexto para la conversación. Configurable por
/// `NEXUM_MEMORY_MAX_TOKENS`.
pub const PRESUPUESTO_TOKENS_DEFAULT: u32 = 400;

/// Caracteres por token, medido — no adivinado.
///
/// `llama-tokenize` con el tokenizador real de Qwen3-0.6B sobre los textos de
/// inyección reales dio entre **3,365** y 4,05 chars/token (7 muestras, español
/// con términos técnicos).
///
/// Se usa **3,3**, por DEBAJO del mínimo observado, para que la estimación se
/// pase de tokens y nunca se quede corta: subestimar el costo es exactamente lo
/// que rompe un presupuesto. El primer intento usó 3,4 —el mínimo redondeado— y
/// el test de esta misma sección lo agarró: estimaba 952 donde el tokenizador
/// real contó 961.
const CHARS_POR_TOKEN: f32 = 3.3;

pub fn estimar_tokens(texto: &str) -> u32 {
    (texto.chars().count() as f32 / CHARS_POR_TOKEN).ceil() as u32
}

fn env_f32(clave: &str, default: f32) -> f32 {
    std::env::var(clave)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| *v >= 0.0 && *v <= 1.0)
        .unwrap_or(default)
}

fn env_u32(clave: &str, default: u32) -> u32 {
    std::env::var(clave)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default)
}

pub fn umbral_relativo() -> f32 {
    env_f32("NEXUM_MEMORY_UMBRAL", UMBRAL_RELATIVO_DEFAULT)
}

pub fn presupuesto_tokens() -> u32 {
    env_u32("NEXUM_MEMORY_MAX_TOKENS", PRESUPUESTO_TOKENS_DEFAULT)
}

/// Una entrada candidata. Mínimo indispensable: no se arrastra el registro
/// entero del store para que esta capa no dependa de su forma.
#[derive(Debug, Clone)]
pub struct Candidata {
    pub id: String,
    pub contenido: String,
    /// `None` = el backend no supo rankear (camino LIKE sin FTS5).
    pub relevancia: Option<f32>,
}

/// Qué se inyectó y qué quedó afuera. **Las dos mitades son resultado**: sin
/// saber qué se descartó no se puede diagnosticar una respuesta rara.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seleccion {
    pub texto: String,
    pub tokens: u32,
    pub incluidas: Vec<String>,
    pub fuera_por_umbral: usize,
    pub fuera_por_presupuesto: usize,
    /// Ninguna entrada traía relevancia: no se inyecta nada.
    pub sin_ranking: bool,
}

impl Seleccion {
    pub fn vacia(&self) -> bool {
        self.incluidas.is_empty()
    }
}

/// Elige qué entra al prompt.
///
/// Orden de las reglas, que importa:
/// 1. **Sin ranking no se inyecta.** Si el backend no supo ordenar, no hay con
///    qué separar lo relevante de lo que apenas comparte una palabra. Inyectar
///    a ciegas es peor que no inyectar.
/// 2. Se corta por umbral relativo al mejor de esta consulta.
/// 3. Se llenan tokens de mayor a menor relevancia hasta el techo.
pub fn seleccionar(candidatas: &[Candidata], umbral: f32, presupuesto: u32) -> Seleccion {
    if candidatas.is_empty() {
        return Seleccion::default();
    }
    if candidatas.iter().all(|c| c.relevancia.is_none()) {
        return Seleccion {
            sin_ranking: true,
            ..Default::default()
        };
    }

    let mut orden: Vec<&Candidata> = candidatas
        .iter()
        .filter(|c| c.relevancia.is_some())
        .collect();
    orden.sort_by(|a, b| {
        b.relevancia
            .unwrap_or(0.0)
            .partial_cmp(&a.relevancia.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mejor = orden[0].relevancia.unwrap_or(0.0);
    let corte = mejor * umbral;

    let mut sel = Seleccion::default();
    let mut lineas: Vec<String> = Vec::new();
    for c in orden {
        let rel = c.relevancia.unwrap_or(0.0);
        if rel < corte {
            sel.fuera_por_umbral += 1;
            continue;
        }
        let linea = format!("- {}", c.contenido);
        let candidato = if lineas.is_empty() {
            linea.clone()
        } else {
            format!("{}\n{}", lineas.join("\n"), linea)
        };
        let t = estimar_tokens(&candidato);
        if t > presupuesto {
            // Entrada entera afuera. Cortarla por el medio puede invertir lo
            // que decía.
            sel.fuera_por_presupuesto += 1;
            continue;
        }
        lineas.push(linea);
        sel.incluidas.push(c.id.clone());
        sel.tokens = t;
    }
    sel.texto = lineas.join("\n");
    sel
}

/// Punto de entrada del turno: consulta, selecciona y deja traza.
///
/// **Nunca falla hacia arriba.** Cualquier problema —sidecar caído, timeout,
/// respuesta rara— devuelve una selección vacía y el turno sigue sin memoria.
/// Al revés no: un cuelgue en el camino de chat ya costó una semana en este
/// proyecto, y agregar una dependencia de red sincrónica a cada turno sin
/// degradación sería reintroducirlo.
///
/// El transporte ya acota: 80 ms de conexión y 800 ms de operación
/// (`client::OP_BUDGET`), así que el peor caso está limitado por construcción.
pub fn preparar(consulta: &str, scope_type: &str, scope_id: &str) -> Seleccion {
    let inicio = std::time::Instant::now();
    let resp = match super::client::recall(consulta, scope_type, scope_id) {
        Ok(r) if r.ok => r,
        otro => {
            // Se traza el fallo: una respuesta sin memoria por sidecar caído y
            // una sin memoria porque nada era relevante se ven IGUAL desde
            // afuera, y hay que poder distinguirlas al diagnosticar.
            let motivo = match otro {
                Err(e) => format!("{e:?}"),
                _ => "respuesta ok=false".to_string(),
            };
            nexum_agent::turn_log::log_memory_degradado(
                &motivo,
                inicio.elapsed().as_millis(),
            );
            return Seleccion::default();
        }
    };
    let candidatas: Vec<Candidata> = resp
        .results
        .iter()
        .map(|e| Candidata {
            id: e.id.clone(),
            contenido: e.content.clone(),
            relevancia: e.relevance,
        })
        .collect();
    let sel = seleccionar(&candidatas, umbral_relativo(), presupuesto_tokens());
    nexum_agent::turn_log::log_memory_inject(
        candidatas.len(),
        &sel.incluidas,
        sel.tokens,
        sel.fuera_por_umbral,
        sel.fuera_por_presupuesto,
        sel.sin_ranking,
        inicio.elapsed().as_millis(),
    );
    sel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, rel: Option<f32>, texto: &str) -> Candidata {
        Candidata {
            id: id.into(),
            contenido: texto.into(),
            relevancia: rel,
        }
    }

    #[test]
    fn sin_ranking_no_se_inyecta_nada() {
        // LA regla: sin con qué medir relevancia, inyectar a ciegas es peor que
        // no inyectar.
        let s = seleccionar(&[c("a", None, "algo"), c("b", None, "otra cosa")], 0.6, 400);
        assert!(s.sin_ranking);
        assert!(s.vacia());
        assert!(s.texto.is_empty());
    }

    #[test]
    fn el_umbral_es_relativo_al_mejor_de_esta_consulta() {
        // El caso medido con 8 memorias: q="Nico" daba 0.60/0.58/0.54. Todas
        // parecidas y todas flojas — pero "flojas" sólo se sabe comparándolas
        // entre sí, no contra un número fijo.
        let bajos = [
            c("1", Some(0.60), "Nico prefiere espanol"),
            c("2", Some(0.58), "no hacer push sin OK"),
            c("3", Some(0.54), "Nico estudia ciberdefensa"),
        ];
        let s = seleccionar(&bajos, 0.6, 400);
        // Con umbral relativo, 0.58 y 0.54 superan 0.60*0.6=0.36: entran las 3.
        assert_eq!(s.incluidas.len(), 3, "scores parejos ⇒ entran juntos");

        // Y con un mejor claramente superior, los flojos quedan afuera.
        let disparejos = [
            c("1", Some(3.85), "el runtime activo es el binario Rust"),
            c("2", Some(0.95), "decision de arquitectura de memoria"),
        ];
        let s2 = seleccionar(&disparejos, 0.6, 400);
        assert_eq!(s2.incluidas, vec!["1"], "0.95 < 3.85*0.6");
        assert_eq!(s2.fuera_por_umbral, 1);
    }

    /// Un umbral ABSOLUTO calibrado con 8 memorias falla con 200. Este test fija
    /// que el corte se mueve con la consulta, que es lo que un número fijo no
    /// puede hacer.
    #[test]
    fn el_corte_se_adapta_al_corpus() {
        let chico = [c("a", Some(0.60), "x"), c("b", Some(0.30), "y")];
        let grande = [c("a", Some(5.88), "x"), c("b", Some(2.90), "y")];
        // Mismo umbral, misma proporción interna ⇒ misma decisión, aunque los
        // valores absolutos difieran en un orden de magnitud.
        assert_eq!(
            seleccionar(&chico, 0.6, 400).incluidas.len(),
            seleccionar(&grande, 0.6, 400).incluidas.len()
        );
    }

    #[test]
    fn el_presupuesto_recorta_entradas_enteras() {
        let largo = "a".repeat(1000); // ~295 tokens
        let cands = [
            c("1", Some(5.0), &largo),
            c("2", Some(4.9), &largo),
            c("3", Some(4.8), &largo),
        ];
        let s = seleccionar(&cands, 0.6, 400);
        assert_eq!(s.incluidas.len(), 1, "sólo entra la primera");
        assert!(s.fuera_por_presupuesto >= 1);
        assert!(s.tokens <= 400, "tokens={}", s.tokens);
        // Y lo que entró está entero: nunca media memoria.
        assert!(s.texto.contains(&largo));
    }

    #[test]
    fn nunca_supera_el_presupuesto() {
        let cands: Vec<Candidata> = (0..50)
            .map(|i| c(&i.to_string(), Some(5.0 - i as f32 * 0.01), &"palabra ".repeat(40)))
            .collect();
        let s = seleccionar(&cands, 0.6, 400);
        assert!(s.tokens <= 400, "tokens={} > 400", s.tokens);
        assert_eq!(estimar_tokens(&s.texto), s.tokens);
    }

    #[test]
    fn la_estimacion_de_tokens_no_se_queda_corta() {
        // Medido con llama-tokenize sobre el texto real: 3234 chars → 961
        // tokens. La estimación tiene que dar >= eso, nunca menos.
        let texto = "x".repeat(3234);
        assert!(
            estimar_tokens(&texto) >= 961,
            "estimó {} para 3234 chars; medido real 961",
            estimar_tokens(&texto)
        );
    }

    #[test]
    fn sin_candidatas_no_hay_seleccion_ni_ruido() {
        let s = seleccionar(&[], 0.6, 400);
        assert!(s.vacia() && !s.sin_ranking && s.texto.is_empty());
    }
}
