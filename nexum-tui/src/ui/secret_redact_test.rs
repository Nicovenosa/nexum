use super::redact_secrets;

#[test]
fn test_redacta_sk_token_con_fingerprint() {
    let input = "la key es sk-proj-AbCdEf1234567890GhIjKlx4Kp y funciona";
    let out = redact_secrets(input);
    assert!(!out.contains("AbCdEf1234567890"), "cuerpo del token fuera: {out}");
    assert!(out.contains("sk-…x4Kp"), "fingerprint prefijo+últimos4: {out}");
}

#[test]
fn test_redacta_tp_token() {
    let input = "token del plan: tp-9f8e7d6c5b4a3210abcd";
    let out = redact_secrets(input);
    assert!(!out.contains("9f8e7d6c5b4a3210"), "{out}");
    assert!(out.contains("tp-…abcd"), "{out}");
}

#[test]
fn test_redacta_apikey_json_de_settings() {
    // El caso real del E2E: el modelo imprimió .peri/settings.json entero.
    let input = r#"{"id":"opencode_zen","apiKey":"sk-oczen-REALKEY-abcdef123456","baseUrl":"https://opencode.ai/zen/v1"}"#;
    let out = redact_secrets(input);
    assert!(!out.contains("REALKEY"), "la key del settings no puede verse: {out}");
    assert!(out.contains("baseUrl"), "el resto del JSON se conserva: {out}");
}

#[test]
fn test_redacta_api_key_env_style() {
    let input = "export ZAI_CODING_API_KEY=zk-1234567890abcdefGHIJ";
    let out = redact_secrets(input);
    assert!(!out.contains("1234567890abcdef"), "{out}");
}

#[test]
fn test_redacta_authorization_bearer() {
    let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.payload";
    let out = redact_secrets(input);
    assert!(!out.contains("eyJhbGciOiJIUzI1NiI"), "{out}");
    assert!(out.contains("[REDACTED…"), "{out}");
}

#[test]
fn test_redacta_access_y_refresh_token() {
    let input = "access_token: AAAA1111bbbb2222cccc\nrefresh_token = RRRR3333dddd4444eeee";
    let out = redact_secrets(input);
    assert!(!out.contains("AAAA1111bbbb2222"), "{out}");
    assert!(!out.contains("RRRR3333dddd4444"), "{out}");
}

#[test]
fn test_redacta_callback_url_code_y_state() {
    let input = "el callback fue http://localhost:1455/auth/callback?code=SECRETCODE123&state=SECRETSTATE456";
    let out = redact_secrets(input);
    assert!(!out.contains("SECRETCODE123"), "{out}");
    assert!(!out.contains("SECRETSTATE456"), "{out}");
    assert!(out.contains("localhost:1455/auth/callback"), "la parte no sensible queda: {out}");
}

#[test]
fn test_no_redacta_texto_normal_espanol() {
    let input = "La arquitectura del proyecto es sólida: auditoría completa, \
                 filosóficamente coherente. El módulo provider_auth_manager.py \
                 maneja los estados not_configured y login_required.";
    assert_eq!(redact_secrets(input), input, "texto normal pasa intacto");
}

#[test]
fn test_no_redacta_sha_de_git() {
    // SHA hex (minúscula+dígito, sin mayúscula) — no debe tocarse.
    let input = "commit 34de9b0f57e33b5aa4e0a21bb293d3c41d8f5e12 en main";
    assert_eq!(redact_secrets(input), input, "los SHA de git no son secrets");
}

#[test]
fn test_redacta_token_generico_largo_mixto() {
    let input = "valor: dGhpc0lzQVZlcnlMb25nU2VjcmV0VG9rZW4xMjM0NTY3ODkw fin";
    let out = redact_secrets(input);
    assert!(
        !out.contains("dGhpc0lzQVZlcnlMb25nU2VjcmV0"),
        "token base64 largo redactado: {out}"
    );
    assert!(out.ends_with("fin"), "{out}");
}

#[test]
fn test_idempotente() {
    let input = r#"apiKey: "sk-something-very-secret-12345678""#;
    let once = redact_secrets(input);
    let twice = redact_secrets(&once);
    assert_eq!(once, twice, "correr dos veces no cambia el resultado");
}

#[test]
fn test_no_redacta_palabra_secretos_en_prosa() {
    let input = "No hay secretos en los logs y la password no se imprime nunca.";
    let out = redact_secrets(input);
    assert!(out.contains("secretos"), "{out}");
    // "password no se imprime" — "no" tiene 2 chars (< mínimo 8), no se toca.
    assert!(out.contains("password no se imprime"), "{out}");
}
