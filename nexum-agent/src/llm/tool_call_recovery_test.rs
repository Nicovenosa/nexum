//! Tests del recuperador de tool calls en texto.
//!
//! El caso que lo originó está primero y textual: es la respuesta real de
//! `qwen2.5:1.5b` a "!listá los archivos del directorio actual".

use super::*;

/// El inventario REAL de herramientas de Nexum. Que los tests usen los nombres
/// verdaderos importa: con nombres inventados, un test puede "pasar" describiendo
/// un mundo que no existe.
const TOOLS: &[&str] = &["Read", "Glob", "Grep", "folder_operations", "Write"];

// ─── El caso que originó esto, y lo que en realidad decía ─────────────────────

#[test]
fn el_tool_call_envuelto_en_markdown_se_recupera() {
    // La forma exacta que emitió qwen2.5:1.5b, con un nombre que sí existe.
    let texto = "```json\n{\"name\": \"folder_operations\", \"arguments\": {}}\n```";
    let calls = recover_tool_calls(texto, TOOLS);
    assert_eq!(calls.len(), 1, "el tool call estaba ahí, sólo tapado: {texto}");
    assert_eq!(calls[0].name, "folder_operations");
    assert_eq!(calls[0].arguments, serde_json::json!({}));
}

#[test]
fn el_caso_original_pedía_una_tool_que_no_existe_y_por_eso_no_se_rescata() {
    // La respuesta real fue `{"name": "ListFiles", "arguments": {}}`. Estaba
    // bien formada, pero `ListFiles` NO es una herramienta de Nexum — el
    // inventario es Read/Glob/Grep/folder_operations/Write/Edit/TodoWrite/
    // AskUserQuestion. El modelo no erró el formato: erró el nombre.
    //
    // Destapar el bloque no alcanza para ese caso, y no debe alcanzar:
    // ejecutar un nombre inventado es peor que no ejecutar nada.
    let texto = "```json\n{\"name\": \"ListFiles\", \"arguments\": {}}\n```";
    assert!(
        recover_tool_calls(texto, TOOLS).is_empty(),
        "un nombre alucinado no se ejecuta por más bien formado que esté"
    );
}

// ─── Liberal al aceptar ───────────────────────────────────────────────────────

#[test]
fn acepta_el_bloque_sin_etiqueta_de_lenguaje() {
    let calls = recover_tool_calls("```\n{\"name\":\"Glob\"}\n```", TOOLS);
    assert_eq!(calls.len(), 1);
}

#[test]
fn acepta_json_pelado_sin_bloque() {
    let calls = recover_tool_calls("{\"name\":\"Glob\",\"arguments\":{}}", TOOLS);
    assert_eq!(calls.len(), 1);
}

#[test]
fn acepta_prosa_alrededor_del_bloque() {
    let texto = "Voy a listar los archivos:\n\n```json\n{\"name\": \"folder_operations\", \
                 \"arguments\": {}}\n```\n\nDespués te cuento.";
    let calls = recover_tool_calls(texto, TOOLS);
    assert_eq!(calls.len(), 1, "la prosa no invalida el tool call");
}

#[test]
fn acepta_los_alias_de_arguments_que_usan_los_modelos() {
    for campo in ["arguments", "parameters", "input", "args"] {
        let texto = format!("{{\"name\":\"Read\",\"{campo}\":{{\"path\":\"a.txt\"}}}}");
        let calls = recover_tool_calls(&texto, TOOLS);
        assert_eq!(calls.len(), 1, "{campo} debe aceptarse");
        assert_eq!(calls[0].arguments, serde_json::json!({"path": "a.txt"}));
    }
}

#[test]
fn acepta_argumentos_serializados_como_string() {
    // OpenAI-style: arguments viene como string JSON, no como objeto.
    let texto = "{\"name\":\"Read\",\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}";
    let calls = recover_tool_calls(texto, TOOLS);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, serde_json::json!({"path": "a.txt"}));
}

#[test]
fn acepta_un_array_de_llamadas() {
    let texto = "```json\n[{\"name\":\"Glob\"},{\"name\":\"Write\",\
                 \"arguments\":{\"path\":\"a\"}}]\n```";
    let calls = recover_tool_calls(texto, TOOLS);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].arguments, serde_json::json!({"path": "a"}));
}

#[test]
fn acepta_la_cerca_sin_cerrar() {
    // El modelo se quedó sin tokens antes del cierre. Es exactamente el caso
    // que hay que tolerar, no el que hay que castigar.
    let calls = recover_tool_calls("```json\n{\"name\":\"Glob\"}", TOOLS);
    assert_eq!(calls.len(), 1);
}

#[test]
fn una_tool_sin_argumentos_no_es_un_descarte() {
    let calls = recover_tool_calls("{\"name\":\"Glob\"}", TOOLS);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, serde_json::json!({}));
}

// ─── Estricto al emitir ───────────────────────────────────────────────────────

#[test]
fn un_nombre_que_no_es_una_tool_disponible_nunca_se_ejecuta() {
    // Sin este filtro, cualquier texto con forma de tool call sería una
    // ejecución. Es la diferencia entre tolerar un formato y obedecer a un
    // string que vino en la respuesta.
    let texto = "```json\n{\"name\": \"rm_rf_todo\", \"arguments\": {}}\n```";
    assert!(recover_tool_calls(texto, TOOLS).is_empty());
}

#[test]
fn sin_tools_habilitadas_no_se_recupera_nada() {
    let texto = "```json\n{\"name\": \"Glob\", \"arguments\": {}}\n```";
    assert!(recover_tool_calls(texto, &[]).is_empty());
}

#[test]
fn json_que_no_es_un_tool_call_se_ignora() {
    for texto in [
        "```json\n{\"resultado\": 42}\n```",
        "```json\n[1, 2, 3]\n```",
        "```json\n{\"name\": \"\"}\n```",
        "```json\n{\"name\": 42}\n```",
    ] {
        assert!(
            recover_tool_calls(texto, TOOLS).is_empty(),
            "no debería recuperar nada de: {texto}"
        );
    }
}

#[test]
fn arguments_que_no_es_objeto_descarta_la_llamada() {
    // Medio tool call no es un tool call: ejecutarlo con argumentos inventados
    // es peor que no ejecutarlo.
    let texto = "{\"name\":\"Read\",\"arguments\":\"no soy json\"}";
    assert!(recover_tool_calls(texto, TOOLS).is_empty());
}

#[test]
fn una_respuesta_de_texto_normal_no_produce_llamadas() {
    let texto = "Hola! Los archivos del directorio son main.rs, lib.rs y Cargo.toml.";
    assert!(recover_tool_calls(texto, TOOLS).is_empty());
}

#[test]
fn el_codigo_que_no_es_json_no_rompe_nada() {
    let texto = "```rust\nfn main() { println!(\"hola\"); }\n```";
    assert!(recover_tool_calls(texto, TOOLS).is_empty());
}

#[test]
fn texto_vacio_no_produce_llamadas() {
    assert!(recover_tool_calls("", TOOLS).is_empty());
    assert!(recover_tool_calls("   \n  ", TOOLS).is_empty());
}
