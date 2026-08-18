//! Interpretación de una descripción de voz en lenguaje natural.
//!
//! "una voz masculina grave y metálica, pausada" → parámetros del motor.
//!
//! # Por qué NO hay un LLM en este camino
//!
//! Se midió el 2026-08-02 (`docs/voice/MEDICION-INTERPRETE-DESCRIPCION.md`).
//! Qwen3-0.6B acertó **2 de 6** casos de diseño y **0 de 6** en holdout, con un
//! modo de fallo específico: `speed` colapsa al valor del ejemplo del prompt
//! —1.0— diga el pedido "pausada", "lenta" o "rapidísima". En un caso devolvió
//! `pitch: "cavernosa"`, fuera del enum que el propio prompt declaraba.
//!
//! Un mapeo léxico sacó 1 de 6 en el mismo holdout. **Los dos fracasan.** Pero
//! fracasan distinto, y esa diferencia es todo el diseño:
//!
//! ```text
//! léxico:  "cavernosa" → SIN CUBRIR              sabe que no sabe → puede preguntar
//! modelo:  "cavernosa" → pitch: "cavernosa"      valor plausible y equivocado
//!          "sin apuro" → speed: 1.0              en silencio
//! ```
//!
//! Generar una voz cuesta MINUTOS. Un parámetro adivinado mal cuesta esos
//! minutos, una voz que no se parece a lo pedido, y un usuario que no sabe por
//! qué. Preguntar cuesta una línea.
//!
//! Por eso la laguna es **parte del resultado**, no un error: [`Interpretacion`]
//! devuelve lo que entendió Y lo que no, y el que llama pregunta antes de gastar
//! los minutos.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pitch {
    MuyGrave,
    Grave,
    Medio,
    Agudo,
    MuyAgudo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Energia {
    Baja,
    Media,
    Alta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Genero {
    Masculina,
    Femenina,
    Neutra,
}

/// Las dimensiones que una descripción puede fijar. Sirve para nombrar la
/// laguna: "no entendí qué velocidad querés" es accionable, "no entendí" no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Pitch,
    Velocidad,
    Energia,
    Genero,
}

impl Dimension {
    /// La pregunta concreta, con las opciones reales del motor.
    ///
    /// Preguntar "¿qué querés decir?" devuelve otra frase que tampoco vamos a
    /// entender. Preguntar con las opciones cierra el lazo en un paso.
    pub fn pregunta(&self) -> &'static str {
        match self {
            Dimension::Pitch => "¿Qué tan grave la querés? (muy grave · grave · media · aguda · muy aguda)",
            Dimension::Velocidad => "¿A qué ritmo? (muy lenta · lenta · normal · rápida · muy rápida)",
            Dimension::Energia => "¿Con cuánta energía? (tranquila · normal · enérgica)",
            Dimension::Genero => "¿Voz masculina, femenina o neutra?",
        }
    }
}

impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Dimension::Pitch => "tono",
            Dimension::Velocidad => "velocidad",
            Dimension::Energia => "energía",
            Dimension::Genero => "género",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parametros {
    pub pitch: Option<Pitch>,
    pub velocidad: Option<f32>,
    pub energia: Option<Energia>,
    pub genero: Option<Genero>,
}

/// Lo entendido Y lo no entendido. Las dos mitades son resultado.
#[derive(Debug, Clone)]
pub struct Interpretacion {
    pub parametros: Parametros,
    /// Dimensiones que la descripción parecía querer fijar y no se pudieron
    /// mapear. **Vacío no significa "entendí todo"**: significa que nada quedó
    /// a medias. Una descripción que no menciona el género no deja laguna.
    pub sin_cubrir: Vec<Dimension>,
}

impl Interpretacion {
    /// Si hay laguna, no se genera: se pregunta. Generar cuesta minutos.
    pub fn puede_generar(&self) -> bool {
        self.sin_cubrir.is_empty()
    }

    /// Las preguntas a hacer, en orden.
    pub fn preguntas(&self) -> Vec<&'static str> {
        self.sin_cubrir.iter().map(|d| d.pregunta()).collect()
    }
}

/// Normaliza para el matcheo: minúsculas, sin tildes.
fn normalizar(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' | 'ü' => 'u',
            c => c,
        })
        .collect()
}

/// Marcas de que la descripción QUISO hablar de una dimensión.
///
/// Separado del léxico que resuelve el valor: es lo que permite distinguir "no
/// lo mencionó" (no hay laguna) de "lo mencionó y no lo entendí" (hay laguna).
/// Sin esta distinción, toda descripción corta parecería incompleta y Nexum
/// preguntaría de más.
const SENAL: &[(Dimension, &[&str])] = &[
    (
        Dimension::Pitch,
        &["grave", "agud", "tono", "timbre", "voz de", "cavernos", "chillon", "profund"],
    ),
    (
        Dimension::Velocidad,
        &["lent", "rapid", "pausad", "despacio", "apuro", "acelerad", "ritmo", "corr", "arrastr"],
    ),
    (
        Dimension::Energia,
        &["energic", "enrgic", "tranquil", "calm", "suave", "fuerza", "chispa", "animad", "vivaz", "susurr"],
    ),
    (Dimension::Genero, &["masculin", "femenin", "hombre", "mujer", "neutr", "androgin", "senora", "senor"]),
];

const LEX_PITCH: &[(&[&str], Pitch)] = &[
    (&["muy grave", "bien grave", "gravisim"], Pitch::MuyGrave),
    (&["muy agud", "agudisim"], Pitch::MuyAgudo),
    (&["grave", "profund", "timbre bajo"], Pitch::Grave),
    (&["agud", "chillon", "fino"], Pitch::Agudo),
    (&["normal", "neutr", "medi"], Pitch::Medio),
];

const LEX_VELOCIDAD: &[(&[&str], f32)] = &[
    (&["rapidisim", "muy rapid", "velocisim"], 1.4),
    (&["muy lent", "lentisim"], 0.6),
    (&["rapid", "acelerad", "ligero", "agil"], 1.2),
    (&["pausad", "lent", "despacio", "sin apuro", "sin correr"], 0.8),
    (&["normal"], 1.0),
];

const LEX_ENERGIA: &[(&[&str], Energia)] = &[
    (&["muy energic", "energic", "vivaz", "animad", "chispa"], Energia::Alta),
    (&["tranquil", "calm", "suave", "seren", "relajad", "susurr", "sin fuerza"], Energia::Baja),
    (&["normal", "neutr"], Energia::Media),
];

const LEX_GENERO: &[(&[&str], Genero)] = &[
    (&["masculin", "hombre", "varon", "senor "], Genero::Masculina),
    (&["femenin", "mujer", "senora"], Genero::Femenina),
    (&["neutr", "androgin"], Genero::Neutra),
];

fn buscar<T: Copy>(d: &str, lex: &[(&[&str], T)]) -> Option<T> {
    lex.iter()
        .find(|(claves, _)| claves.iter().any(|k| d.contains(k)))
        .map(|(_, v)| *v)
}

/// Interpreta una descripción. **Nunca adivina**: lo que no mapea queda en
/// `sin_cubrir`.
pub fn interpretar(descripcion: &str) -> Interpretacion {
    let d = normalizar(descripcion);
    let parametros = Parametros {
        pitch: buscar(&d, LEX_PITCH),
        velocidad: buscar(&d, LEX_VELOCIDAD),
        energia: buscar(&d, LEX_ENERGIA),
        genero: buscar(&d, LEX_GENERO),
    };
    let mut sin_cubrir: Vec<Dimension> = Vec::new();
    for (dim, marcas) in SENAL {
        let mencionada = marcas.iter().any(|m| d.contains(m));
        let resuelta = match dim {
            Dimension::Pitch => parametros.pitch.is_some(),
            Dimension::Velocidad => parametros.velocidad.is_some(),
            Dimension::Energia => parametros.energia.is_some(),
            Dimension::Genero => parametros.genero.is_some(),
        };
        if mencionada && !resuelta {
            sin_cubrir.push(*dim);
        }
    }
    // No entender NADA es la laguna más grande, y era la que se colaba: sin
    // ninguna señal reconocida, `sin_cubrir` quedaba vacío y `puede_generar()`
    // daba true con los cuatro parámetros en None. Eso genera una voz por
    // defecto y se la presenta como si fuera la pedida — el mismo fallo
    // silencioso que el modelo, por otro camino.
    let entendio_algo = parametros.pitch.is_some()
        || parametros.velocidad.is_some()
        || parametros.energia.is_some()
        || parametros.genero.is_some();
    if !entendio_algo {
        sin_cubrir = vec![
            Dimension::Pitch,
            Dimension::Velocidad,
            Dimension::Energia,
            Dimension::Genero,
        ];
    }

    Interpretacion { parametros, sin_cubrir }
}

// ─── Validador de coherencia ──────────────────────────────────────────────────

/// Verifica que unos parámetros se correspondan con lo que se pidió.
///
/// **Es independiente de [`interpretar`] a propósito.** Validar la salida del
/// propio mapeador sería circular: el valor de este chequeo es atrapar una
/// propuesta equivocada venga de donde venga —un ajuste manual del usuario, un
/// perfil importado, o un modelo que alguna vez proponga valores—.
///
/// Corre ANTES de generar. Es la Validación 2 ("que cumpla el pedido") aplicada
/// antes de gastar los minutos, no después de gastarlos.
pub fn verificar_coherencia(descripcion: &str, p: &Parametros) -> Result<(), Vec<String>> {
    let d = normalizar(descripcion);
    let mut fallos = Vec::new();

    let pide_grave = ["grave", "profund", "cavernos", "timbre bajo"]
        .iter()
        .any(|k| d.contains(k));
    let pide_agudo = ["agud", "chillon", "fino"].iter().any(|k| d.contains(k));
    if pide_grave && matches!(p.pitch, Some(Pitch::Agudo) | Some(Pitch::MuyAgudo)) {
        fallos.push("se pidió grave y el tono quedó agudo".to_string());
    }
    if pide_agudo && matches!(p.pitch, Some(Pitch::Grave) | Some(Pitch::MuyGrave)) {
        fallos.push("se pidió aguda y el tono quedó grave".to_string());
    }

    let pide_lento = ["lent", "pausad", "despacio", "sin apuro", "sin correr", "arrastr"]
        .iter()
        .any(|k| d.contains(k));
    let pide_rapido = ["rapid", "acelerad", "apurad"].iter().any(|k| d.contains(k));
    if let Some(v) = p.velocidad {
        if pide_lento && v >= 1.0 {
            fallos.push(format!("se pidió lenta y la velocidad quedó en {v}"));
        }
        if pide_rapido && v <= 1.0 {
            fallos.push(format!("se pidió rápida y la velocidad quedó en {v}"));
        }
    }

    let pide_tranquilo = ["tranquil", "calm", "suave", "seren", "susurr", "sin fuerza"]
        .iter()
        .any(|k| d.contains(k));
    let pide_energico = ["energic", "vivaz", "animad", "chispa"].iter().any(|k| d.contains(k));
    if pide_tranquilo && p.energia == Some(Energia::Alta) {
        fallos.push("se pidió tranquila y la energía quedó alta".to_string());
    }
    if pide_energico && p.energia == Some(Energia::Baja) {
        fallos.push("se pidió enérgica y la energía quedó baja".to_string());
    }

    if fallos.is_empty() { Ok(()) } else { Err(fallos) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapea_lo_que_conoce() {
        let i = interpretar("una voz masculina grave y metálica, pausada");
        assert_eq!(i.parametros.pitch, Some(Pitch::Grave));
        assert_eq!(i.parametros.velocidad, Some(0.8));
        assert_eq!(i.parametros.genero, Some(Genero::Masculina));
        assert!(i.puede_generar(), "{:?}", i.sin_cubrir);
    }

    #[test]
    fn no_inventa_lo_que_no_se_pidio() {
        // "grave" no dice nada del género. Inventarlo es lo que hacía el modelo.
        let i = interpretar("bien grave y lenta");
        assert_eq!(i.parametros.genero, None);
        assert!(i.puede_generar(), "no mencionar algo no es una laguna");
    }

    /// LA propiedad: lo que no entiende se declara, no se rellena.
    #[test]
    fn lo_que_no_entiende_queda_sin_cubrir_y_no_genera() {
        // Descripción de la que no reconoce NADA: no puede pasar por entendida.
        let i = interpretar("que hable como si estuviera contando un chiste");
        assert_eq!(i.parametros, Parametros::default(), "no mapeó nada, y está bien");
        assert!(
            i.sin_cubrir.contains(&Dimension::Energia),
            "sin nada entendido, la laguna es total: {:?}",
            i.sin_cubrir
        );
        assert!(!i.puede_generar(), "con laguna no se generan minutos de audio");
        assert!(!i.preguntas().is_empty());
    }

    #[test]
    fn la_pregunta_trae_las_opciones_concretas() {
        // Preguntar "¿qué querés decir?" devuelve otra frase que tampoco vamos
        // a entender.
        assert!(Dimension::Velocidad.pregunta().contains("rápida"));
        assert!(Dimension::Genero.pregunta().contains("neutra"));
    }

    #[test]
    fn el_validador_atrapa_la_contradiccion_del_modelo() {
        // El caso medido: "pausada" con speed 1.0, que Qwen3 devolvió 4 de 6
        // veces. El validador es independiente del mapeador justamente para
        // poder atrapar propuestas de otra fuente.
        let p = Parametros {
            pitch: Some(Pitch::Grave),
            velocidad: Some(1.0),
            energia: None,
            genero: Some(Genero::Masculina),
        };
        let err = verificar_coherencia("masculina grave y pausada", &p)
            .expect_err("1.0 no es pausada");
        assert!(err[0].contains("se pidió lenta"), "{err:?}");
    }

    #[test]
    fn el_validador_atrapa_grave_con_tono_agudo() {
        let p = Parametros { pitch: Some(Pitch::MuyAgudo), ..Default::default() };
        assert!(verificar_coherencia("una voz cavernosa", &p).is_err());
    }

    #[test]
    fn el_validador_acepta_lo_coherente() {
        let i = interpretar("femenina, aguda y muy enérgica, que hable rápido");
        verificar_coherencia("femenina, aguda y muy enérgica, que hable rápido", &i.parametros)
            .expect("lo que el mapeador produce tiene que pasar su propio validador");
    }

    /// Las descripciones del holdout: no se piden aciertos, se pide que no
    /// mienta. Cada una o resuelve, o declara la laguna — nunca inventa.
    #[test]
    fn en_el_holdout_o_resuelve_o_declara_la_laguna() {
        for desc in [
            "como un locutor de radio antigua, con voz cavernosa y sin apuro",
            "quiero que suene chillona y acelerada",
            "algo susurrado, casi sin fuerza, arrastrando las palabras",
            "voz de señora mayor, dulce, sin correr",
            "tipo robot militar: cortante, seco, timbre bajo",
            "que hable como si estuviera contando un chiste, con chispa",
        ] {
            let i = interpretar(desc);
            if i.puede_generar() {
                verificar_coherencia(desc, &i.parametros)
                    .unwrap_or_else(|e| panic!("'{desc}' se dio por entendida pero es incoherente: {e:?}"));
            } else {
                assert!(!i.preguntas().is_empty(), "'{desc}' sin laguna nombrada");
            }
        }
    }
}
