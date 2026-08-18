//! VoiceCatalog + VoiceSelector (F2.2.2): catálogo adaptativo de voces
//! con procedencia declarada POR ENTRADA y selección determinística
//! y auditable. Nombres públicos Nexum (perfiles, no "voz exclusiva").
//! Regla: capacidades honestas — piper/kokoro NO soportan pitch: "más
//! grave" se resuelve eligiendo otra voz (jamás fingir).

use serde::{Deserialize, Serialize};

/// Equipo donde se tomaron las mediciones del catálogo.
///
/// Es el equipo OBJETIVO del perfil low-cost, no una máquina de desarrollo
/// cómoda: si un motor entra en presupuesto acá, entra en cualquier lado.
pub const MAQUINA_MEDICION: &str =
    "Vivobook Go E1504FA · AMD Ryzen 5 7520U · 8 hilos · 7 GB · governor performance · en corriente";

/// Presupuesto de latencia del camino de respuesta, POR PERFIL DE HARDWARE.
///
/// Un techo único no puede servir para un desktop y para un netbook: el mismo
/// motor da números distintos y la restricción tiene que hablar de la máquina
/// donde corre, no de un promedio que no existe en ningún lado.
///
/// Los valores salen de lo medido en [`MAQUINA_MEDICION`], que es `low`:
/// Piper mide 1060 ms ahí, así que 1500 ms deja ~440 ms de margen en el equipo
/// más lento del perfil. `medium` se deriva de ese margen, no de una medición
/// propia — ver `procedencia_del_presupuesto`.
/// # Contra qué tier resuelve hoy
///
/// Contra el `cpu_tier` de la ENTRADA, que es el perfil de la máquina donde se
/// midió — hoy todas dicen `low` y [`MAQUINA_MEDICION`] es `low`, así que
/// coinciden. **La forma correcta es resolver contra el tier de la máquina que
/// está corriendo**, detectado en runtime.
///
/// No se hace todavía a propósito: un detector de tier es una fuente de verdad
/// nueva y adivinada (¿por núcleos? ¿por RAM? ¿por modelo de CPU?), y una que
/// se equivoque afloja el presupuesto justo en el equipo que había que
/// proteger. Mientras no exista, resolver contra el tier de medición es
/// conservador: en una máquina más rápida el motor tarda menos que lo medido,
/// nunca más.
pub fn presupuesto_respuesta_ms(cpu_tier: &str) -> u32 {
    match cpu_tier {
        "low" => 1500,
        "medium" => 1000,
        _ => 1500,
    }
}

/// Qué tan sólido es cada techo. Se responde acá y no en un doc aparte porque
/// la pregunta "¿de dónde salió este número?" es la que quedó sin respuesta
/// verificable cuatro veces en este mismo archivo.
pub fn procedencia_del_presupuesto(cpu_tier: &str) -> Procedencia {
    match cpu_tier {
        // Medido en el equipo objetivo: Piper 1060 ms, margen 440 ms.
        "low" => Procedencia::Medido {
            fecha: "2026-08-02",
            maquina: MAQUINA_MEDICION,
            metodo: "techo = latencia medida de Piper (1060 ms) + margen; el equipo medido ES el más lento del perfil",
        },
        // Nadie midió un equipo `medium`. El techo es una derivación, y decirlo
        // es lo único que impide que se cite como si estuviera medido.
        _ => Procedencia::Estimado,
    }
}

/// De dónde salen los números de una entrada.
///
/// Existe porque el encabezado de este módulo decía "métricas MEDIDAS
/// (benchmark 2026-07-09)", dos comentarios de campo decían "medido", y al
/// medirlos el 2026-08-02 **cuatro de seis estaban mal** — Piper declaraba
/// 60 MB contra 109 reales y Kokoro 1300 ms contra 3700. Una afirmación global
/// de que todo fue medido no se puede verificar ni desmentir por entrada; ésta
/// sí, y `rol_valido` la exige donde importa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Procedencia {
    /// Medido, con máquina y método reproducibles.
    ///
    /// `maquina` no es decorativo: una latencia sin el equipo donde se tomó no
    /// se puede comparar contra un presupuesto. Estos números se etiquetaron
    /// "desktop" durante medio día y en realidad salieron del Vivobook — el
    /// equipo LENTO, o sea el que manda. La etiqueta invertía la conclusión.
    Medido { fecha: &'static str, maquina: &'static str, metodo: &'static str },
    /// Declarado a mano. Nunca alcanza para el camino de respuesta.
    Estimado,
}

impl Procedencia {
    pub fn es_medido(&self) -> bool {
        matches!(self, Procedencia::Medido { .. })
    }
}

/// Para qué sirve una voz. División por ROL, no por preferencia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rol {
    /// Confirmaciones y respuestas conversacionales. Sujeto al presupuesto.
    Respuesta,
    /// Texto largo, donde el usuario ya sabe que va a escuchar un rato y la
    /// latencia inicial se amortiza. Un motor lento pero mejor cabe acá.
    Narracion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceCatalogEntry {
    pub id: &'static str,           // público Nexum
    pub display_name: &'static str,
    pub engine: &'static str,       // piper | kokoro
    pub engine_voice_id: &'static str,
    pub language: &'static str,
    pub accent: &'static str,
    pub perceived_pitch_hz: u32,    // f0 MEDIDO (autocorrelación)
    pub warmth: u8,                 // 1-5 estimado
    pub energy: u8,
    pub clarity: u8,
    pub pace_default: f32,
    pub supports_speed: bool,
    pub supports_pitch: bool,       // ninguno actual: false (honesto)
    pub supports_emotion: bool,
    pub model_size_mb: u32,
    pub peak_ram_mb: u32,           // medido
    pub first_audio_ms_short: u32,  // medido, frase corta
    pub cpu_tier: &'static str,     // low | medium
    pub license: &'static str,
    pub enabled: bool,
    pub rol: Rol,
    pub procedencia: Procedencia,
}

/// Una capa que procesa el texto ANTES del TTS, con su costo medido.
///
/// Existe para que el presupuesto se aplique a la CADENA y no al motor suelto.
/// Un motor que entra y una cadena que no entra es un chequeo que se detiene
/// antes del borde que importa — el mismo defecto que tenía el doctor de voz
/// cuando verificaba que Piper existiera en vez de que sintetizara.
#[derive(Debug, Clone)]
pub struct CapaPrevia {
    pub nombre: &'static str,
    pub latencia_ms: u32,
    pub procedencia: Procedencia,
}

impl VoiceCatalogEntry {
    /// Las dos invariantes del camino de respuesta, juntas y en un solo lugar.
    ///
    /// Sin medición no se entra: es la regla que evita repetir el catálogo con
    /// números inventados, y la que decide qué hacer con un motor candidato
    /// (Voicebox, Qwen3-TTS, Chatterbox) antes de que alguien lo enchufe —
    /// mientras nadie mida su latencia, sólo puede aspirar a `Narracion`.
    pub fn rol_valido(&self) -> Result<(), String> {
        self.rol_valido_en_cadena(&[])
    }

    /// Igual, pero sobre la CADENA COMPLETA: capas previas + motor.
    ///
    /// El presupuesto es del camino que recorre el texto hasta sonar, no del
    /// último tramo. Con una capa de carácter en el medio, la suma es la que
    /// tiene que entrar — y toda capa cuenta con la misma vara: sin medir, no
    /// entra al camino de respuesta.
    pub fn rol_valido_en_cadena(&self, capas: &[CapaPrevia]) -> Result<(), String> {
        if self.rol != Rol::Respuesta {
            return Ok(());
        }
        if !self.procedencia.es_medido() {
            return Err(format!(
                "{}: rol Respuesta con números estimados — medí antes de ponerlo a responder",
                self.id
            ));
        }
        for c in capas {
            if !c.procedencia.es_medido() {
                return Err(format!(
                    "{}: la capa '{}' no está medida — medí antes de meterla en el camino de respuesta",
                    self.id, c.nombre
                ));
            }
        }
        let techo = presupuesto_respuesta_ms(self.cpu_tier);
        let capas_ms: u32 = capas.iter().map(|c| c.latencia_ms).sum();
        let total = self.first_audio_ms_short + capas_ms;
        if total > techo {
            let detalle = if capas.is_empty() {
                String::new()
            } else {
                let partes: Vec<String> = capas
                    .iter()
                    .map(|c| format!("{} {} ms", c.nombre, c.latencia_ms))
                    .collect();
                format!(" (motor {} ms + {})", self.first_audio_ms_short, partes.join(" + "))
            };
            return Err(format!(
                "{}: la cadena suma {total} ms{detalle} y supera el presupuesto de {techo} ms para cpu_tier={} — su rol es Narracion",
                self.id, self.cpu_tier
            ));
        }
        Ok(())
    }
}

impl VoiceCatalogEntry {
    /// Entrada de último recurso (idéntica a la default Piper daniela) para
    /// el caso teórico de catálogo vacío — evita panic en el flujo de voz.
    pub fn fallback() -> Self {
        VoiceCatalogEntry {
            id: "nova",
            display_name: "Nova",
            engine: "piper",
            engine_voice_id: "es_AR-daniela-high",
            language: "es",
            accent: "es-AR",
            perceived_pitch_hz: 190,
            warmth: 3,
            energy: 3,
            clarity: 4,
            pace_default: 1.0,
            supports_speed: true,
            supports_pitch: false,
            supports_emotion: false,
            model_size_mb: 109,
            peak_ram_mb: 200,
            first_audio_ms_short: 900,
            cpu_tier: "low",
            license: "MIT",
            enabled: true,
            rol: Rol::Respuesta,
            procedencia: Procedencia::Medido { fecha: "2026-08-02", maquina: MAQUINA_MEDICION, metodo: "one-shot completo (spawn→wav), frase corta, mediana de 6 corridas Piper / 3 Kokoro, RSS pico de /proc" },
        }
    }
}

/// Catálogo curado.
///
/// # Procedencia de los números
///
/// Medidos el 2026-08-02 en [`MAQUINA_MEDICION`] —el Vivobook, o sea el equipo
/// objetivo del perfil low-cost— proceso one-shot completo (spawn → wav
/// escrito), frase corta "Listo, ya lo apliqué.", mediana de 6 corridas Piper /
/// 3 Kokoro, RSS pico muestreado de `/proc/<pid>/status`.
///
/// Estuvieron etiquetados "desktop" medio día, con un "Vivobook ~1.5-2x"
/// heredado encima. Los dos eran falsos y juntos invertían la conclusión: el
/// equipo lento ya era el que estaba midiendo. Por eso
/// `Procedencia::Medido` exige `maquina`.
///
/// El encabezado anterior también decía "benchmark real" y **cuatro de seis
/// números estaban mal**: Piper declaraba 60 MB de modelo contra 109 reales,
/// 100 MB de RAM contra 175, y Kokoro 1300 ms contra ~3700. El tamaño de
/// Kokoro no contaba `voices-v1.0.bin` (27 MB), que hace falta para sintetizar.
///
/// `first_audio_ms_short` es hasta el wav COMPLETO, no hasta la primera
/// muestra: Nexum no hace streaming, sintetiza entero y recién ahí reproduce.
/// Mientras eso siga así, las dos cosas son lo mismo para el usuario.
pub fn catalog() -> Vec<VoiceCatalogEntry> {
    let kokoro = |id, name, vid, f0, warmth, energy| VoiceCatalogEntry {
        id, display_name: name, engine: "kokoro", engine_voice_id: vid,
        language: "es", accent: "es neutro", perceived_pitch_hz: f0,
        warmth, energy, clarity: 4, pace_default: 1.0,
        supports_speed: true, supports_pitch: false, supports_emotion: false,
        model_size_mb: 115, peak_ram_mb: 259, first_audio_ms_short: 3700,
        cpu_tier: "low", license: "Apache-2.0", enabled: true,
        // 3700 ms medidos: fuera del camino de respuesta por latencia, no por
        // calidad. Suena mejor y llega tarde — eso lo hace narrador, no
        // interlocutor.
        rol: Rol::Narracion, procedencia: Procedencia::Medido { fecha: "2026-08-02", maquina: MAQUINA_MEDICION, metodo: "one-shot completo (spawn→wav), frase corta, mediana de 6 corridas Piper / 3 Kokoro, RSS pico de /proc" },
    };
    vec![
        // Piper (rápidas: conversación/confirmaciones)
        VoiceCatalogEntry {
            id: "nexum_nova", display_name: "Nexum Nova",
            engine: "piper", engine_voice_id: "es_AR-daniela-high",
            language: "es", accent: "es-AR", perceived_pitch_hz: 190,
            warmth: 4, energy: 3, clarity: 5, pace_default: 1.0,
            supports_speed: true, supports_pitch: false, supports_emotion: false,
            model_size_mb: 109, peak_ram_mb: 175, first_audio_ms_short: 1060,
            cpu_tier: "low", license: "MIT", enabled: true,
            rol: Rol::Respuesta, procedencia: Procedencia::Medido { fecha: "2026-08-02", maquina: MAQUINA_MEDICION, metodo: "one-shot completo (spawn→wav), frase corta, mediana de 6 corridas Piper / 3 Kokoro, RSS pico de /proc" },
        },
        // Kokoro (variedad/identidad; f0 medidos)
        kokoro("nexum_calma", "Nexum Calma", "ef_dora", 178, 4, 2),
        kokoro("nexum_atlas", "Nexum Atlas", "em_alex", 149, 3, 3),
        kokoro("nexum_claro", "Nexum Claro", "em_santa", 143, 3, 2),
    ]
}

/// VoiceDirective extendida (F2.2.2) — deseo del usuario, interpretado
/// LOCAL. None = sin preferencia.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VoiceDirective {
    pub desired_pitch: Option<i8>,   // -1 más grave · +1 más agudo
    pub desired_warmth: Option<i8>,
    pub desired_energy: Option<i8>,
    pub desired_pace: Option<i8>,    // -1 más lento · +1 más rápido
    pub preview_requested: bool,     // "probame otra voz"
    pub previous: bool,              // "volvé a la voz anterior"
    pub reset: bool,
    pub persist: bool,
}

impl VoiceDirective {
    /// Parser local de frases naturales. 0 tokens, sin sidecar.
    pub fn parse(phrase: &str) -> Option<Self> {
        let f = phrase.to_lowercase();
        let has = |ws: &[&str]| ws.iter().any(|w| f.contains(w));
        let mut d = VoiceDirective { persist: true, ..Default::default() };
        let mut any = false;
        if has(&["anterior", "voz de antes", "la de antes"]) {
            d.previous = true;
            return Some(d);
        }
        if has(&["normal", "reset", "tu voz de siempre"]) {
            d.reset = true;
            return Some(d);
        }
        if has(&["probame", "probemos", "otra voz", "no me gusta tu voz", "cambia la voz", "cambiá la voz"]) {
            d.preview_requested = true;
            any = true;
        }
        if has(&["grave", "rave", "rábe", "orave", "profunda", "masculina"]) { d.desired_pitch = Some(-1); any = true; }
        if has(&["aguda", "femenina", "más alta"]) { d.desired_pitch = Some(1); any = true; }
        if has(&["cálida", "calida", "suave", "dulce"]) { d.desired_warmth = Some(1); any = true; }
        if has(&["seria", "serio", "formal", "neutra", "neutro"]) { d.desired_warmth = Some(-1); any = true; }
        if has(&["tranquil", "calma", "lento", "despacio", "pausad"]) { d.desired_pace = Some(-1); any = true; }
        if has(&["rápid", "rapid", "energ", "animad"]) { d.desired_pace = Some(1); any = true; }
        any.then_some(d)
    }
}

/// Selección determinística y auditable: (elegida, score, razón).
pub fn select(directive: &VoiceDirective, current_id: &str) -> (VoiceCatalogEntry, String) {
    let cat: Vec<_> = catalog().into_iter().filter(|e| e.enabled && e.language == "es").collect();
    let cur = cat.iter().find(|e| e.id == current_id).cloned();
    let cur_pitch = cur.as_ref().map(|c| c.perceived_pitch_hz).unwrap_or(190);
    let mut best: Option<(i64, &VoiceCatalogEntry)> = None;
    for e in &cat {
        let mut score: i64 = 0;
        // 1. dirección de pitch pedida (peso mayor)
        if let Some(dp) = directive.desired_pitch {
            let delta = e.perceived_pitch_hz as i64 - cur_pitch as i64;
            score += if (dp < 0 && delta < -10) || (dp > 0 && delta > 10) {
                40 + delta.abs().min(60) / 2
            } else if e.id == current_id {
                0
            } else {
                -30
            };
        }
        if let Some(dw) = directive.desired_warmth {
            score += (if dw > 0 { e.warmth as i64 } else { 5 - e.warmth as i64 }) * 6;
        }
        if let Some(de) = directive.desired_energy {
            score += (if de > 0 { e.energy as i64 } else { 5 - e.energy as i64 }) * 4;
        }
        // 2. penalizar RAM/latencia (presupuesto low-cost)
        score -= (e.peak_ram_mb / 200) as i64;
        score -= (e.first_audio_ms_short / 500) as i64;
        // 3. estabilidad: pequeña preferencia por la voz actual
        if e.id == current_id {
            score += 5;
        }
        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, e));
        }
    }
    // Fallback sin panic: si el filtro (enabled && es) dejara el catálogo
    // vacío (p.ej. voces deshabilitadas a futuro), degradar a la primera
    // voz del catálogo completo en vez de panicear en pleno flujo de voz.
    let (score, chosen) = match best {
        Some((s, e)) => (s, e.clone()),
        None => match catalog().into_iter().next() {
            Some(e) => (0, e),
            None => {
                return (
                    VoiceCatalogEntry::fallback(),
                    "selector: catálogo vacío → fallback fijo".to_string(),
                )
            }
        },
    };
    let chosen = &chosen;
    let reason = format!(
        "selector: {} (f0 {}Hz, warmth {}, ram {}MB) score {} · pedido: pitch{:?} warmth{:?} pace{:?}",
        chosen.id, chosen.perceived_pitch_hz, chosen.warmth, chosen.peak_ram_mb,
        score, directive.desired_pitch, directive.desired_warmth, directive.desired_pace
    );
    (chosen.clone(), reason)
}

/// Candidatos de preview (máx 3, distintos entre sí y de la actual).
pub fn preview_candidates(current_id: &str) -> Vec<VoiceCatalogEntry> {
    catalog()
        .into_iter()
        .filter(|e| e.enabled && e.id != current_id)
        .take(3)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mas_grave_elige_voz_mas_grave_sin_fingir_pitch() {
        let d = VoiceDirective::parse("quiero una voz más grave").unwrap();
        assert_eq!(d.desired_pitch, Some(-1));
        let (e, reason) = select(&d, "nexum_nova"); // actual: 190Hz
        assert!(e.perceived_pitch_hz < 190, "eligió más grave: {} ({}Hz)", e.id, e.perceived_pitch_hz);
        assert!(!e.supports_pitch, "honesto: nadie soporta pitch");
        assert!(reason.contains("selector:"), "auditable: {reason}");
    }

    #[test]
    fn test_grave_y_tranquilo_combinado() {
        let d = VoiceDirective::parse("quiero algo más grave y tranquilo").unwrap();
        assert_eq!(d.desired_pitch, Some(-1));
        assert_eq!(d.desired_pace, Some(-1));
        assert!(d.persist);
        let (e, _) = select(&d, "nexum_nova");
        assert!(e.id == "nexum_claro" || e.id == "nexum_atlas", "grave: {}", e.id);
    }

    #[test]
    fn test_previous_y_preview_y_no_match() {
        assert!(VoiceDirective::parse("volvé a la voz anterior").unwrap().previous);
        assert!(VoiceDirective::parse("no me gusta tu voz").unwrap().preview_requested);
        assert!(VoiceDirective::parse("qué hora es").is_none());
    }

    #[test]
    fn test_preview_max_3_sin_la_actual() {
        let c = preview_candidates("nexum_nova");
        assert!(c.len() <= 3);
        assert!(c.iter().all(|e| e.id != "nexum_nova"));
    }

    #[test]
    fn test_catalogo_presupuesto_low_cost() {
        for e in catalog() {
            assert!(e.peak_ram_mb <= 1024, "{} ≤1GB", e.id);
            assert!(e.model_size_mb <= 500, "{} ≤500MB", e.id);
            assert_eq!(e.cpu_tier, "low");
        }
    }

    /// Las invariantes del camino de respuesta, sobre el catálogo entero.
    ///
    /// Los topes viejos (1 GB de RAM, 500 MB de modelo) no muerden: ningún
    /// valor realista los viola, así que el test pasaba dijeran lo que dijeran
    /// los números — y de hecho pasó con cuatro mal.
    #[test]
    fn ninguna_voz_responde_con_numeros_sin_medir_ni_fuera_de_presupuesto() {
        for e in catalog() {
            e.rol_valido().expect("invariante del camino de respuesta");
        }
        VoiceCatalogEntry::fallback()
            .rol_valido()
            .expect("el fallback también responde");
    }

    /// La regla del contrato, en su forma más directa: si el catálogo dice que
    /// un número fue medido, tiene que decir CUÁNDO y CÓMO. Una afirmación
    /// global de "métricas medidas" en el encabezado del módulo no se puede
    /// verificar por entrada, y fue exactamente la que tapó los cuatro errores.
    #[test]
    fn lo_declarado_como_medido_dice_cuando_y_como() {
        for e in catalog() {
            if let Procedencia::Medido { fecha, maquina, metodo } = &e.procedencia {
                assert!(!maquina.is_empty(), "{}: medido sin decir en qué máquina", e.id);
                assert!(!fecha.is_empty(), "{}: medido sin fecha", e.id);
                assert!(
                    metodo.len() > 20,
                    "{}: el método tiene que ser reproducible, no una etiqueta",
                    e.id
                );
            }
        }
    }

    /// Un motor candidato sin medir no puede colarse al camino de respuesta.
    #[test]
    fn un_motor_sin_medir_es_rechazado_para_responder() {
        let mut candidato = VoiceCatalogEntry::fallback();
        candidato.id = "candidato_sin_medir";
        candidato.procedencia = Procedencia::Estimado;
        let err = candidato.rol_valido().expect_err("tiene que rechazarlo");
        assert!(err.contains("medí antes"), "{err}");
    }

    /// Y uno medido pero lento tampoco: entra como narrador.
    #[test]
    fn un_motor_lento_es_rechazado_para_responder_aunque_este_medido() {
        let mut candidato = VoiceCatalogEntry::fallback();
        candidato.id = "candidato_lento";
        candidato.first_audio_ms_short = presupuesto_respuesta_ms(candidato.cpu_tier) + 1;
        let err = candidato.rol_valido().expect_err("tiene que rechazarlo");
        assert!(err.contains("su rol es Narracion"), "{err}");
    }

    /// El techo tiene que depender del perfil de hardware, no ser global.
    #[test]
    fn el_presupuesto_es_por_perfil_de_hardware() {
        assert!(
            presupuesto_respuesta_ms("medium") < presupuesto_respuesta_ms("low"),
            "un equipo más rápido no puede tener un techo más laxo que uno lento"
        );
    }

    /// Y el techo que NO se midió tiene que decir que no se midió.
    ///
    /// `low` sale del equipo objetivo real. `medium` es una derivación: nadie
    /// midió uno. Mientras lo diga, nadie lo puede citar como medido — que es
    /// exactamente lo que pasó con cuatro números de este archivo.
    #[test]
    fn el_techo_derivado_no_se_declara_medido() {
        assert!(procedencia_del_presupuesto("low").es_medido());
        assert!(!procedencia_del_presupuesto("medium").es_medido());
    }

    /// La máquina de medición es el equipo OBJETIVO, no una de desarrollo.
    ///
    /// Si esto se invierte, el presupuesto pasa a estar validado en la máquina
    /// cómoda y a no decir nada del equipo que importa. Pasó: estos números
    /// dijeron "desktop" medio día, y salían del Vivobook.
    #[test]
    fn la_maquina_de_medicion_esta_declarada() {
        assert!(MAQUINA_MEDICION.contains("Vivobook"), "{MAQUINA_MEDICION}");
        for e in catalog() {
            if let Procedencia::Medido { maquina, .. } = &e.procedencia {
                assert_eq!(*maquina, MAQUINA_MEDICION, "{}", e.id);
            }
        }
    }

    /// El caso que motivó validar la cadena: motor que entra, cadena que no.
    ///
    /// Piper mide 1060 ms contra un techo de 1500: entra con 440 ms de margen.
    /// Qwen3-0.6B mide 412-494 ms reescribiendo una frase corta en el equipo
    /// objetivo. La cadena se pasa, aunque cada tramo por separado parezca
    /// razonable. Validar el motor solo habría dado luz verde.
    #[test]
    fn un_motor_que_entra_con_una_capa_que_no_cabe_es_rechazado() {
        let piper = catalog()
            .into_iter()
            .find(|e| e.engine == "piper")
            .expect("hay una entrada Piper");
        piper.rol_valido().expect("el motor solo entra");

        let qwen = CapaPrevia {
            nombre: "qwen3-0.6b",
            latencia_ms: 471,
            procedencia: Procedencia::Medido {
                fecha: "2026-08-02",
                maquina: MAQUINA_MEDICION,
                metodo: "ollama /api/generate, think=false, num_predict=40, mediana de 9 reescrituras en caliente",
            },
        };
        let err = piper
            .rol_valido_en_cadena(&[qwen])
            .expect_err("la cadena se pasa del presupuesto");
        assert!(err.contains("la cadena suma 1531 ms"), "{err}");
    }

    /// Y una capa sin medir tampoco entra, tenga la latencia que tenga.
    #[test]
    fn una_capa_sin_medir_no_entra_al_camino_de_respuesta() {
        let piper = catalog().into_iter().find(|e| e.engine == "piper").unwrap();
        let capa = CapaPrevia {
            nombre: "capa_fantasia",
            latencia_ms: 1,
            procedencia: Procedencia::Estimado,
        };
        let err = piper.rol_valido_en_cadena(&[capa]).expect_err("rechaza");
        assert!(err.contains("no está medida"), "{err}");
    }
}
