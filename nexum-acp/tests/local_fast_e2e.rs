//! NEXUM_LOCAL_FAST_DEMO_V1 — E2E temporal del camino DIRECT_CHAT.
//!
//! Mide, contra Ollama loopback real, lo que DIRECT_CHAT entrega:
//!   1. el **prompt mínimo** exacto al que el executor rutea (`local_micro`),
//!      contrastado con el prompt completo (~4k tokens);
//!   2. un roundtrip real con ese prompt + **cero tools**, capturando los
//!      **tokens de entrada reales** (usage de Ollama), la **duración** y el
//!      **provider/model trace**.
//!
//! Ignorado por defecto (golpea Ollama). Ejecutar:
//!   NEXUM_LOCAL_FAST=1 cargo test -p nexum-acp --test local_fast_e2e \
//!       -- --ignored --nocapture
//!
//! El flujo TUI → TaskEnvelopeV1 → Hormiguero → ACP se preserva: DIRECT_CHAT
//! sólo decide, dentro del executor, este system prompt mínimo + tools vacías.

use nexum_acp::prompt::{build_system_prompt, PromptFeatures};
use nexum_agent::llm::types::LlmRequest;
use nexum_agent::llm::{BaseModel, ChatOpenAI};
use nexum_agent::messages::BaseMessage;

const OLLAMA_BASE: &str = "http://127.0.0.1:11434/v1";
const CANARY: &str = "Respondé únicamente: NEXUM_LOCAL_FAST_OK";

fn model() -> String {
    std::env::var("NEXUM_REPRO_MODEL").unwrap_or_else(|_| "qwen2.5:1.5b".to_string())
}

/// Estimación conservadora de tokens a partir de caracteres (~4 chars/token).
fn est_tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

#[tokio::test]
#[ignore]
async fn direct_chat_minimal_prompt_and_real_roundtrip() {
    // 1) Prompt mínimo (DIRECT_CHAT) vs completo — medición de contexto.
    let minimal = build_system_prompt(None, ".", PromptFeatures::local_micro(), &[], None, Some("es"));
    let full = build_system_prompt(None, ".", PromptFeatures::detect(), &[], None, Some("es"));

    let min_chars = minimal.chars().count();
    let full_chars = full.chars().count();
    println!(
        "LOCALFAST\tprompt\tminimal_chars\t{}\tminimal_est_tokens\t{}\tfull_chars\t{}\tfull_est_tokens\t{}",
        min_chars,
        est_tokens(min_chars),
        full_chars,
        est_tokens(full_chars)
    );
    // El prompt mínimo debe preservar identidad/objetivo/salida y ser mucho
    // menor que el completo.
    assert!(minimal.contains("Nexum") || minimal.contains("Nexum Agent"),
        "el prompt mínimo debe conservar la identidad");
    assert!(min_chars < full_chars, "el prompt mínimo debe ser menor que el completo");

    // 2) Roundtrip real con el prompt mínimo + CERO tools.
    let adapter = ChatOpenAI::new("ollama", model())
        .with_base_url(OLLAMA_BASE)
        .with_local_cpu_profile();
    assert!(!adapter.supports_streaming(), "perfil local ⇒ non-stream");

    let request = LlmRequest::new(vec![BaseMessage::human(CANARY)])
        .with_system(minimal.clone())
        .with_max_tokens(64);
    // Sin .with_tools(...) ⇒ cero tool schemas expuestos (DIRECT_CHAT).

    let t0 = std::time::Instant::now();
    let resp = adapter.invoke(request).await.expect("roundtrip DIRECT_CHAT debe completar");
    let dt = t0.elapsed().as_secs_f64();

    let text = format!("{:?}", resp.message);
    let ok = text.contains("NEXUM_LOCAL_FAST_OK");
    let real_input_tokens = resp.usage.as_ref().map(|u| u.input_tokens).unwrap_or(0);

    println!(
        "LOCALFAST\tcanary\tprovider_id\tollama_local\tmodel_id\t{}\treal_input_tokens\t{}\tduration_s\t{:.2}\ttools\t0\tcanary_ok\t{}\tresult\t{}",
        model(),
        real_input_tokens,
        dt,
        ok,
        if ok && dt < 30.0 { "PASS" } else { "FAIL" }
    );

    // Gates de la misión.
    assert!(ok, "la respuesta debe contener el canary");
    assert!(dt < 30.0, "duración {dt:.2}s debe ser < 30s (mínimo técnico)");
    assert!(
        real_input_tokens > 0 && real_input_tokens <= 1200,
        "tokens de entrada reales ({real_input_tokens}) deben ser <= 1200"
    );
    if dt <= 20.0 {
        println!("LOCALFAST\tdemo_target\tinteractive_simple_turn_seconds <= 20\tMET");
    } else {
        println!("LOCALFAST\tdemo_target\tinteractive_simple_turn_seconds <= 20\tNOT_MET (dt={dt:.2}s, pero < 30 técnico)");
    }
}
