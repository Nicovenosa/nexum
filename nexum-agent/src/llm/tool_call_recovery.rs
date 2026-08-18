//! Recuperación de tool calls que el modelo emitió como texto.
//!
//! Un modelo local chico (`qwen2.5:1.5b`) respondió a "listá los archivos" con
//! un tool call perfectamente formado —`{"name": "ListFiles", "arguments": {}}`—
//! envuelto en un bloque de código markdown. El adaptador lo trató como
//! respuesta final: la herramienta nunca corrió, y el turn log lo registró como
//! `tool:"none", parseable:false`. El problema no era el modelo: era que nadie
//! destapaba el bloque antes de mirar adentro.
//!
//! **Sé liberal al aceptar y estricto al emitir.** Liberal: se destapan bloques
//! de código con o sin lenguaje, se acepta el JSON pelado, un objeto o un array,
//! y los nombres de campo que los modelos usan en la práctica (`arguments`,
//! `parameters`, `input`, `args`).
//!
//! Estricto: **un nombre que no corresponde a una herramienta disponible no se
//! ejecuta jamás.** Sin ese filtro, cualquier texto que se parezca a un tool call
//! —el modelo explicando cómo se ve uno, por ejemplo— se convertiría en una
//! ejecución. Es la diferencia entre tolerar un formato y obedecer a un string.

/// Un tool call rescatado del texto, ya validado contra las tools disponibles.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Nombres de campo bajo los que los modelos ponen los argumentos.
const CAMPOS_DE_ARGUMENTOS: &[&str] = &["arguments", "parameters", "input", "args"];

/// Devuelve los fragmentos de texto que pueden contener JSON: el contenido de
/// cada bloque ```…``` más, si no hubo bloques, el texto entero.
///
/// El delimitador se busca a mano en vez de con un parser de markdown porque lo
/// único que importa acá es lo que hay entre las cercas.
fn destapar_bloques(texto: &str) -> Vec<&str> {
    let mut fragmentos = Vec::new();
    let mut resto = texto;

    while let Some(inicio) = resto.find("```") {
        let despues = &resto[inicio + 3..];
        // La primera línea puede ser el lenguaje (```json) o vacía (```).
        let cuerpo = match despues.find('\n') {
            Some(nl) => &despues[nl + 1..],
            // ```{...}``` en una sola línea: no hay etiqueta de lenguaje.
            None => despues,
        };
        match cuerpo.find("```") {
            Some(fin) => {
                fragmentos.push(&cuerpo[..fin]);
                resto = &cuerpo[fin + 3..];
            }
            None => {
                // Cerca sin cerrar: el modelo se quedó sin tokens. Igual se
                // intenta, que es justamente el caso que hay que tolerar.
                fragmentos.push(cuerpo);
                break;
            }
        }
    }

    if fragmentos.is_empty() {
        fragmentos.push(texto);
    }
    fragmentos
}

/// Extrae `name` + argumentos de un objeto JSON, si los tiene bien formados.
fn como_llamada(valor: &serde_json::Value) -> Option<RecoveredCall> {
    let obj = valor.as_object()?;
    let name = obj.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    // Los argumentos pueden faltar (una tool sin parámetros): eso es `{}`, no
    // un descarte. Lo que NO se acepta es un `arguments` que no sea objeto.
    let arguments = CAMPOS_DE_ARGUMENTOS
        .iter()
        .find_map(|campo| obj.get(*campo))
        .cloned();
    let arguments = match arguments {
        None => serde_json::json!({}),
        Some(serde_json::Value::Object(m)) => serde_json::Value::Object(m),
        // Algunos modelos serializan los argumentos como string JSON.
        Some(serde_json::Value::String(s)) => match serde_json::from_str(&s) {
            Ok(serde_json::Value::Object(m)) => serde_json::Value::Object(m),
            _ => return None,
        },
        Some(_) => return None,
    };
    Some(RecoveredCall {
        name: name.to_string(),
        arguments,
    })
}

/// Recupera los tool calls que un modelo escribió como texto.
///
/// `tools_disponibles` es el filtro duro: un nombre que no está en esa lista se
/// descarta sin ejecutarse. Si la lista está vacía no se recupera nada — sin
/// herramientas habilitadas no hay nada legítimo que rescatar.
pub fn recover_tool_calls(texto: &str, tools_disponibles: &[&str]) -> Vec<RecoveredCall> {
    if texto.trim().is_empty() || tools_disponibles.is_empty() {
        return Vec::new();
    }

    let mut encontrados = Vec::new();
    for fragmento in destapar_bloques(texto) {
        let fragmento = fragmento.trim();
        if fragmento.is_empty() {
            continue;
        }
        let Ok(valor) = serde_json::from_str::<serde_json::Value>(fragmento) else {
            continue;
        };
        // Un objeto suelto o un array de objetos: los modelos usan las dos.
        let candidatos: Vec<&serde_json::Value> = match &valor {
            serde_json::Value::Array(items) => items.iter().collect(),
            otro => vec![otro],
        };
        for candidato in candidatos {
            if let Some(llamada) = como_llamada(candidato) {
                encontrados.push(llamada);
            }
        }
    }

    // Estricto al emitir: sólo sobrevive lo que corresponde a una herramienta
    // real. Un nombre inventado no es un tool call, es texto.
    encontrados.retain(|c| tools_disponibles.contains(&c.name.as_str()));
    encontrados
}

#[cfg(test)]
#[path = "tool_call_recovery_test.rs"]
mod tests;
