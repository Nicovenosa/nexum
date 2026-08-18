//! Tests del clasificador de flujo DIRECT_CHAT (determinístico, sin red).

use super::{classify, should_direct_chat, FlowClass};

fn is_direct(s: &str) -> bool {
    classify(s) == FlowClass::DirectChat
}

#[test]
fn respond_only_es_direct_chat() {
    assert!(is_direct("Respondé únicamente: NEXUM_LOCAL_FAST_OK"));
    assert!(is_direct("Respondé únicamente con: NEXUM_ACP_E2E_OK"));
    assert!(is_direct("respondé solamente OK"));
    assert!(is_direct("Reply only: PONG"));
    assert!(is_direct("answer only with yes"));
}

#[test]
fn saludos_y_preguntas_triviales_cortas_son_direct_chat() {
    assert!(is_direct("hola"));
    assert!(is_direct("¿Cómo estás?"));
    assert!(is_direct("Decime un número del 1 al 10"));
    assert!(is_direct("¿Cuánto es 2 + 2?"));
}

#[test]
fn prompts_con_tool_o_archivo_son_full_react() {
    assert_eq!(classify("Analizá estos logs y proponé pasos"), FlowClass::FullReact);
    assert_eq!(classify("Buscá mi página de Anytype"), FlowClass::FullReact);
    assert_eq!(classify("Leé el archivo /home/user/x.rs"), FlowClass::FullReact);
    assert_eq!(classify("Ejecutá los tests"), FlowClass::FullReact);
    assert_eq!(classify("Corré git status"), FlowClass::FullReact);
    assert_eq!(classify("Revisá config.toml"), FlowClass::FullReact);
    assert_eq!(classify("fetch https://example.com"), FlowClass::FullReact);
    assert_eq!(classify("implementá el fix"), FlowClass::FullReact);
}

#[test]
fn codigo_o_multilinea_es_full_react() {
    assert_eq!(
        classify("Mirá esto:\n```rust\nfn main() {}\n```"),
        FlowClass::FullReact
    );
    // Multilínea CORTO sin señales ya no escala: puede ser un texto pegado
    // para resumir, no una tarea. La duda cae del lado barato. Para escalar
    // hace falta multilínea + largo, o una señal de intención.
    assert_eq!(
        classify("línea uno\nlínea dos\nlínea tres\nlínea cuatro"),
        FlowClass::DirectChat
    );
}

#[test]
fn prompt_largo_sin_senales_ya_no_escala_por_longitud() {
    // El criterio viejo escalaba por tamaño. Ahora escala por INTENCIÓN: un
    // texto largo sin ninguna señal de tool sigue siendo charla.
    let largo = "a ".repeat(200);
    assert_eq!(classify(&largo), FlowClass::DirectChat);
}

#[test]
fn vacio_va_a_directo_no_al_loop_de_tools() {
    // Un mensaje vacío no tiene nada que resolver con herramientas; mandarlo
    // al loop de ReAct era gastar diez iteraciones sobre la nada.
    assert_eq!(classify(""), FlowClass::DirectChat);
    assert_eq!(classify("   "), FlowClass::DirectChat);
}

#[test]
fn respond_only_gana_aunque_sea_apenas_largo_pero_sin_señales() {
    // Un "respondé únicamente" con una frase objetivo breve sigue siendo DIRECT_CHAT.
    assert!(is_direct(
        "Respondé únicamente con la palabra clave exacta y nada más: LISTO"
    ));
}

#[test]
fn respond_only_con_senal_de_tool_escala_por_seguridad() {
    // Si además pide leer un archivo, NO es chat directo: escala.
    assert_eq!(
        classify("Respondé únicamente con el contenido de /etc/hostname"),
        FlowClass::FullReact
    );
}

// ── Gate de ruteo (should_direct_chat) ─────────────────────────────────────

#[test]
fn gate_off_por_flag_preserva_flujo_completo() {
    // Flag OFF ⇒ nunca DIRECT_CHAT, aunque el prompt sea simple.
    assert!(!should_direct_chat(false, false, "Respondé únicamente: OK"));
}

#[test]
fn gate_con_envelope_explicito_no_rutea() {
    // Con un envelope explícito presente, no pisamos su semántica.
    assert!(!should_direct_chat(true, true, "Respondé únicamente: OK"));
}

#[test]
fn gate_on_sin_envelope_y_prompt_simple_rutea() {
    assert!(should_direct_chat(true, false, "Respondé únicamente: OK"));
    assert!(should_direct_chat(true, false, "hola"));
}

#[test]
fn gate_on_pero_prompt_de_tarea_no_rutea() {
    assert!(!should_direct_chat(true, false, "Analizá estos logs"));
    assert!(!should_direct_chat(true, false, "Leé /home/x/y.rs"));
}

// ─── Escalado por INTENCIÓN, no por longitud ────────────────────────────────

#[test]
fn charla_va_a_directo_aunque_no_tenga_marcador() {
    // Los casos que mandaban "hola nexum" al loop de ReAct.
    for msg in [
        "hola nexum",
        "estás funcionando?",
        "¿estás activo?",
        "¿cuánto es 12 por 4?",
        "buenas, todo bien?",
        "qué modelo sos",
        "gracias!",
        "explicame qué es un mutex",
        "¿cuál es la capital de Francia?",
    ] {
        assert_eq!(
            classify(msg),
            FlowClass::DirectChat,
            "«{msg}» es charla: no necesita tools"
        );
    }
}

#[test]
fn pedidos_con_intencion_real_escalan() {
    for msg in [
        "leé el archivo README.md",
        "corré los tests",
        "buscá en el proyecto dónde está la función classify",
        "borrá /tmp/x",
        "ejecutá cargo build",
        "abrí src/main.rs y decime qué hace",
        "hacé commit de esto",
    ] {
        assert_eq!(
            classify(msg),
            FlowClass::FullReact,
            "«{msg}» pide una acción real"
        );
    }
}

#[test]
fn un_pedido_corto_de_accion_escala_aunque_sea_breve() {
    // 12 caracteres: el umbral viejo lo mandaba a directo.
    assert_eq!(classify("borrá /tmp/x"), FlowClass::FullReact);
}

#[test]
fn una_pregunta_larga_sin_senales_sigue_siendo_charla() {
    // El umbral viejo escalaba cualquier cosa de más de 240 chars.
    let larga = "Che, tengo una duda conceptual: cuando hablamos de concurrencia \
        y de paralelismo mucha gente los usa como sinónimos, pero tengo entendido \
        que no son lo mismo. ¿Me explicás la diferencia con un ejemplo cotidiano, \
        sin meterte en detalles técnicos de ningún lenguaje puntual?";
    assert!(larga.chars().count() > 240, "el caso tiene que ser largo");
    assert_eq!(classify(larga), FlowClass::DirectChat);
}

#[test]
fn el_usuario_puede_forzar_el_camino_con_tools() {
    for msg in ["!hola", "/tools hola", "/agente contame algo"] {
        assert_eq!(
            classify(msg),
            FlowClass::FullReact,
            "«{msg}» fuerza tools explícitamente"
        );
    }
}

#[test]
fn la_duda_cae_del_lado_barato() {
    // Ambiguo: sin señal clara, va a lo barato.
    assert_eq!(classify("y esto cómo lo resolverías?"), FlowClass::DirectChat);
    assert_eq!(classify(""), FlowClass::DirectChat);
}

// ─── Matching por palabra: el falso positivo que la Fase 1 venía a eliminar ──

#[test]
fn palabras_que_CONTIENEN_un_verbo_no_escalan() {
    // Antes se buscaba por substring: "implementación" matcheaba "implementa",
    // "buscador" matcheaba "busca", "testigo" matcheaba "test".
    for msg in [
        "explicame los detalles de implementación de un mutex",
        "qué opinás del buscador de Google",
        "fui testigo de algo raro",
        "me gusta la creatividad de esa solución",
        "contame sobre la instalación eléctrica de mi casa",
        "el corredor de la maratón",
    ] {
        assert_eq!(
            classify(msg),
            FlowClass::DirectChat,
            "«{msg}» es charla: la palabra sólo CONTIENE un verbo de acción"
        );
    }
}

#[test]
fn los_verbos_de_verdad_siguen_escalando() {
    for msg in [
        "implementá el parser",
        "buscá dónde está eso",
        "corré los tests",
        "creá un archivo nuevo",
        "instalá la dependencia",
    ] {
        assert_eq!(classify(msg), FlowClass::FullReact, "«{msg}» pide una acción");
    }
}

#[test]
fn los_fragmentos_siguen_matcheando_por_substring() {
    // Paths y extensiones no son palabras: su sola presencia delata intención.
    for msg in ["mirá src/main.rs", "andá a https://x.com", "el config.toml ese"] {
        assert_eq!(classify(msg), FlowClass::FullReact, "«{msg}» tiene un fragmento");
    }
}


// ─── Polaridad del flag LOCAL_FAST ────────────────────────────────────────────
//
// El flag pasó de opt-in a default ON: el comportamiento sano no puede depender
// de que alguien se acuerde de exportar una variable. Estos tests fijan la
// polaridad nueva — si alguien la vuelve a invertir, fallan acá y no en
// producción con un turno colgado.

#[test]
fn sin_la_variable_el_ruteo_esta_encendido() {
    assert!(
        super::local_fast_enabled_from(None),
        "default ON: una instalación limpia rutea por intención"
    );
}

#[test]
fn la_variable_solo_sirve_para_apagar() {
    for apagado in ["0", "false", "off", "no"] {
        assert!(
            !super::local_fast_enabled_from(Some(apagado)),
            "{apagado} tiene que apagar el ruteo"
        );
    }
}

#[test]
fn los_valores_de_encendido_viejos_siguen_encendiendo() {
    // Quien tenga NEXUM_LOCAL_FAST=1 exportado de antes no debe notar el cambio.
    for encendido in ["1", "true", "on", "yes"] {
        assert!(
            super::local_fast_enabled_from(Some(encendido)),
            "{encendido} tiene que dejar el ruteo encendido"
        );
    }
}

#[test]
fn un_valor_desconocido_no_apaga_el_ruteo() {
    // Falla hacia el comportamiento sano: un typo no devuelve el cuelgue.
    assert!(super::local_fast_enabled_from(Some("quizas")));
    assert!(super::local_fast_enabled_from(Some("")));
}
