//! Redactor de secretos para TODO texto visible del transcript (spec
//! REABRIR CHAT UX §4, 2026-07-06).
//!
//! Contexto: una auditoría generada por el modelo imprimió API keys reales
//! leídas de `.peri/settings.json`. No alcanza con no loguear secrets: nada
//! que parezca una credencial puede llegar RENDERIZADO a la pantalla (ni al
//! clipboard vía copy, que usa el mismo texto ya redactado).
//!
//! Se aplica en los puntos de construcción de los ViewModels (texto del
//! asistente, output de herramientas, streaming bubble) — una sola pasada
//! por texto, sin dependencias (sin regex crate).
//!
//! Qué redacta:
//!  1. Valores de claves conocidas: apiKey/api_key/access_token/
//!     refresh_token/client_secret/management_key/password/authorization/
//!     Bearer → `[REDACTED…xxxx]` (últimos 4 para poder correlacionar).
//!  2. Tokens con prefijo conocido: sk-, tp-, ghp_, github_pat_, xoxb-,
//!     AIza, ya29. → `sk-…x4Kp` (prefijo + últimos 4).
//!  3. Query params sensibles en URLs: code=, state=, token=, key=,
//!     api_key=, access_token= → valor reemplazado por `…`.
//!  4. Tokens genéricos largos (≥36 chars, con mayúscula+minúscula+dígito —
//!     los SHA hex de git NO matchean) → `xx…xxxx`.
//!
//! Falsos positivos aceptados por diseño (safety-first): mejor redactar un
//! string raro de más que imprimir una key real.

/// Charset de cuerpo de token: lo que puede seguir a un prefijo/valor.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '/' | '=')
}

/// Fingerprint `…xxxx` de un token (últimos 4 chars).
fn tail4(token: &str) -> String {
    let tail: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    tail
}

/// Claves cuyo VALOR se redacta (comparación case-insensitive).
const VALUE_KEYS: &[&str] = &[
    "api_key",
    "api-key",
    "apikey",
    "access_token",
    "refresh_token",
    "client_secret",
    "management_key",
    "secret_key",
    "authorization",
    "password",
];

/// Prefijos de token conocidos (el fingerprint conserva el prefijo).
const TOKEN_PREFIXES: &[&str] = &[
    "sk-", "tp-", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AIza", "ya29.",
];

/// Query params sensibles en URLs.
const URL_PARAMS: &[&str] = &[
    "code=",
    "state=",
    "token=",
    "key=",
    "api_key=",
    "apikey=",
    "access_token=",
    "sig=",
];

const MIN_VALUE_LEN: usize = 8;
const MIN_PREFIX_TOKEN_LEN: usize = 10; // prefijo incluido
const MIN_GENERIC_LEN: usize = 36;
const MIN_URL_PARAM_LEN: usize = 6;

/// Redacta secretos en texto visible. Idempotente (correr dos veces no
/// cambia el resultado). Texto normal (español, markdown, código sin
/// credenciales) pasa intacto.
pub fn redact_secrets(text: &str) -> String {
    let step1 = redact_value_keys(text);
    let step2 = redact_prefix_tokens(&step1);
    let step3 = redact_url_params(&step2);
    redact_generic_long_tokens(&step3)
}

/// Paso 1: `apiKey: "sk-real..."` / `password = hunter2secret` → valor fuera.
fn redact_value_keys(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut consumed = 0usize; // byte offset en text

    for (key_pos, key) in find_all_keys(&lower) {
        if key_pos < consumed {
            continue; // solapado con un match anterior ya procesado
        }
        let key_end = key_pos + key.len();
        // Boundary: el char siguiente debe ser separador (evita "secretos").
        let rest = &text[key_end..];
        let mut chars = rest.char_indices().peekable();
        let mut cursor = key_end;
        // Saltar separadores: comillas, espacios, ':', '=', '>'
        while let Some(&(i, c)) = chars.peek() {
            if matches!(c, '"' | '\'' | ' ' | '\t' | ':' | '=' | '>') {
                chars.next();
                cursor = key_end + i + c.len_utf8();
            } else {
                break;
            }
        }
        if cursor == key_end {
            continue; // sin separador → palabra más larga ("secretos"), no es un par clave-valor
        }
        // Capturar valor
        let value_start = cursor;
        let mut value_end = cursor;
        for (i, c) in text[value_start..].char_indices() {
            if is_token_char(c) {
                value_end = value_start + i + c.len_utf8();
            } else {
                break;
            }
        }
        let value = &text[value_start..value_end];
        // "Authorization: Bearer <tok>" — el primer "valor" es Bearer: saltarlo.
        let (value_start, value_end, value) = if value.eq_ignore_ascii_case("bearer") {
            let after = &text[value_end..];
            let skip_ws: usize = after
                .char_indices()
                .take_while(|(_, c)| *c == ' ' || *c == '\t')
                .map(|(_, c)| c.len_utf8())
                .sum();
            let vs = value_end + skip_ws;
            let mut ve = vs;
            for (i, c) in text[vs..].char_indices() {
                if is_token_char(c) {
                    ve = vs + i + c.len_utf8();
                } else {
                    break;
                }
            }
            (vs, ve, &text[vs..ve])
        } else {
            (value_start, value_end, value)
        };
        if value.chars().count() < MIN_VALUE_LEN || value.starts_with("[REDACTED") {
            continue;
        }
        out.push_str(&text[consumed..value_start]);
        out.push_str(&format!("[REDACTED…{}]", tail4(value)));
        consumed = value_end;
    }
    out.push_str(&text[consumed..]);
    out
}

/// Posiciones (byte) de todas las claves conocidas, ordenadas.
fn find_all_keys(lower: &str) -> Vec<(usize, &'static str)> {
    let mut hits: Vec<(usize, &'static str)> = Vec::new();
    for key in VALUE_KEYS {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(key) {
            let abs = from + pos;
            hits.push((abs, key));
            from = abs + key.len();
        }
    }
    hits.sort_by_key(|(p, _)| *p);
    // Si dos claves solapan (apikey dentro de api_key no pasa, pero por las
    // dudas), gana la primera.
    hits
}

/// Paso 2: tokens con prefijo conocido → `sk-…x4Kp`.
fn redact_prefix_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut consumed = 0usize;
    let mut hits: Vec<(usize, &'static str)> = Vec::new();
    for prefix in TOKEN_PREFIXES {
        let mut from = 0;
        while let Some(pos) = text[from..].find(prefix) {
            let abs = from + pos;
            hits.push((abs, prefix));
            from = abs + prefix.len();
        }
    }
    hits.sort_by_key(|(p, _)| *p);
    for (pos, prefix) in hits {
        if pos < consumed {
            continue;
        }
        // Boundary a la izquierda: inicio o char que no es de token
        // (evita matchear "task-" dentro de una palabra, y re-redactar
        // un `sk-…x4Kp` ya emitido: '…' no es token char, pero el cuerpo
        // tras el prefijo sería corto y no pasa el mínimo).
        if pos > 0 {
            if let Some(prev) = text[..pos].chars().last() {
                if is_token_char(prev) {
                    continue;
                }
            }
        }
        let body_start = pos + prefix.len();
        let mut body_end = body_start;
        for (i, c) in text[body_start..].char_indices() {
            if is_token_char(c) {
                body_end = body_start + i + c.len_utf8();
            } else {
                break;
            }
        }
        let full = &text[pos..body_end];
        if full.chars().count() < MIN_PREFIX_TOKEN_LEN {
            continue;
        }
        out.push_str(&text[consumed..pos]);
        out.push_str(&format!("{}…{}", prefix, tail4(full)));
        consumed = body_end;
    }
    out.push_str(&text[consumed..]);
    out
}

/// Paso 3: `?code=SECRET&state=SECRET` → valores fuera.
fn redact_url_params(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut consumed = 0usize;
    let mut hits: Vec<(usize, &'static str)> = Vec::new();
    for param in URL_PARAMS {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(param) {
            let abs = from + pos;
            // Solo dentro de una URL: precedido por '?' o '&'.
            let is_url_param = text[..abs].chars().last().is_some_and(|c| c == '?' || c == '&');
            if is_url_param {
                hits.push((abs, param));
            }
            from = abs + param.len();
        }
    }
    hits.sort_by_key(|(p, _)| *p);
    for (pos, param) in hits {
        if pos < consumed {
            continue;
        }
        let value_start = pos + param.len();
        let mut value_end = value_start;
        for (i, c) in text[value_start..].char_indices() {
            if is_token_char(c) {
                value_end = value_start + i + c.len_utf8();
            } else {
                break;
            }
        }
        if text[value_start..value_end].chars().count() < MIN_URL_PARAM_LEN {
            continue;
        }
        out.push_str(&text[consumed..value_start]);
        out.push('…');
        consumed = value_end;
    }
    out.push_str(&text[consumed..]);
    out
}

/// Paso 4: token genérico largo con mayúscula+minúscula+dígito.
/// Los SHA de git (hex minúscula) y las palabras largas NO matchean.
fn redact_generic_long_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token_start: Option<usize> = None;
    let mut last_end = 0usize;

    let mut flush = |out: &mut String, text: &str, start: usize, end: usize| {
        let token = &text[start..end];
        let n = token.chars().count();
        let has_lower = token.chars().any(|c| c.is_ascii_lowercase());
        let has_upper = token.chars().any(|c| c.is_ascii_uppercase());
        let has_digit = token.chars().any(|c| c.is_ascii_digit());
        if n >= MIN_GENERIC_LEN && has_lower && has_upper && has_digit {
            let head: String = token.chars().take(2).collect();
            out.push_str(&format!("{}…{}", head, tail4(token)));
        } else {
            out.push_str(token);
        }
    };

    for (i, c) in text.char_indices() {
        if is_token_char(c) {
            if token_start.is_none() {
                out.push_str(&text[last_end..i]);
                token_start = Some(i);
            }
        } else if let Some(start) = token_start.take() {
            flush(&mut out, text, start, i);
            last_end = i;
        }
    }
    if let Some(start) = token_start {
        flush(&mut out, text, start, text.len());
    } else {
        out.push_str(&text[last_end..]);
    }
    out
}

#[cfg(test)]
#[path = "secret_redact_test.rs"]
mod tests;
