//! FASE 1 — Repro aislado reqwest ↔ Ollama (diagnóstico micro-fix).
//!
//! Usa EXACTAMENTE el `build_reqwest_client()` del producto (misma versión,
//! features y ClientBuilder) para pinnear DÓNDE se produce el timeout en el
//! camino streaming, y confirmar que el camino non-stream completa < 30 s.
//!
//! Ignorado por defecto (golpea Ollama loopback real). Ejecutar con:
//!   cargo test -p nexum-agent --lib ollama_repro -- --ignored --nocapture
//!
//! No forma parte de la suite normal; no debilita ningún test existente.

use super::build_reqwest_client;
use futures::StreamExt;
use serde_json::json;
use std::time::Instant;

const OLLAMA_BASE: &str = "http://127.0.0.1:11434/v1";

fn repro_model() -> String {
    std::env::var("NEXUM_REPRO_MODEL").unwrap_or_else(|_| "qwen2.5:1.5b".to_string())
}

fn body(model: &str, streaming: bool) -> serde_json::Value {
    // Parametrizable por env para pinnear la falla bajo carga sin recompilar:
    //   NEXUM_REPRO_PROMPT   — prompt del usuario (default: canary trivial)
    //   NEXUM_REPRO_MAXTOK   — max_tokens (default 64)
    //   NEXUM_REPRO_SYSPAD   — nº de repeticiones de padding en el system prompt
    //                          (fuerza prefill largo en CPU) (default 0)
    let prompt = std::env::var("NEXUM_REPRO_PROMPT")
        .unwrap_or_else(|_| "Respondé únicamente: NEXUM_PROVIDER_OK".to_string());
    let max_tok: u32 = std::env::var("NEXUM_REPRO_MAXTOK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let syspad: usize = std::env::var("NEXUM_REPRO_SYSPAD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let mut messages = Vec::new();
    if syspad > 0 {
        // Padding grande para forzar tiempo de prefill (time-to-first-token) alto.
        let pad = "Contexto de referencia irrelevante para la tarea. ".repeat(syspad);
        messages.push(json!({"role": "system", "content": pad}));
    }
    messages.push(json!({"role": "user", "content": prompt}));

    let mut b = json!({
        "model": model,
        "messages": messages,
        "stream": streaming,
        "max_tokens": max_tok
    });
    if streaming && model.to_lowercase().contains("qwen") {
        b["stream_options"] = json!({"include_usage": true});
    }
    b
}

/// Camina la cadena `source()` de un error para clasificarlo sin exponer secretos.
fn source_chain(e: &dyn std::error::Error) -> String {
    let mut out = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        out.push(s.to_string());
        cur = s.source();
    }
    out.join(" <- ")
}

#[tokio::test]
#[ignore]
async fn repro_ollama_non_stream() {
    let model = repro_model();
    let client = build_reqwest_client();
    let url = format!("{OLLAMA_BASE}/chat/completions");
    let start = Instant::now();

    println!("REPRO\tcase\tnon_stream\tmodel\t{model}");
    let send = client
        .post(&url)
        .bearer_auth("ollama")
        .json(&body(&model, false))
        .send()
        .await;
    let headers_at = start.elapsed().as_secs_f64();
    match send {
        Ok(resp) => {
            let status = resp.status();
            let version = format!("{:?}", resp.version());
            println!("REPRO\tnon_stream\theaders_s\t{headers_at:.2}\tstatus\t{status}\thttp\t{version}");
            let text = resp.text().await;
            let total = start.elapsed().as_secs_f64();
            match text {
                Ok(t) => {
                    let ok = t.contains("NEXUM_PROVIDER_OK")
                        || serde_json::from_str::<serde_json::Value>(&t)
                            .ok()
                            .map(|v| {
                                v["choices"][0]["message"]["content"]
                                    .as_str()
                                    .unwrap_or("")
                                    .contains("NEXUM_PROVIDER_OK")
                            })
                            .unwrap_or(false);
                    println!(
                        "REPRO\tnon_stream\tRESULT\tPASS\ttotal_s\t{total:.2}\tbytes\t{}\tcontains_canary\t{ok}",
                        t.len()
                    );
                }
                Err(e) => println!(
                    "REPRO\tnon_stream\tRESULT\tFAIL_BODY\ttotal_s\t{total:.2}\tis_timeout\t{}\tsource\t{}",
                    e.is_timeout(),
                    source_chain(&e)
                ),
            }
        }
        Err(e) => {
            let total = start.elapsed().as_secs_f64();
            println!(
                "REPRO\tnon_stream\tRESULT\tFAIL_SEND\ttotal_s\t{total:.2}\tis_timeout\t{}\tis_connect\t{}\tsource\t{}",
                e.is_timeout(),
                e.is_connect(),
                source_chain(&e)
            );
        }
    }
}

#[tokio::test]
#[ignore]
async fn repro_ollama_stream() {
    let model = repro_model();
    let client = build_reqwest_client();
    let url = format!("{OLLAMA_BASE}/chat/completions");
    let start = Instant::now();

    println!("REPRO\tcase\tstream\tmodel\t{model}");
    let send = client
        .post(&url)
        .bearer_auth("ollama")
        .json(&body(&model, true))
        .send()
        .await;
    let headers_at = start.elapsed().as_secs_f64();
    let resp = match send {
        Ok(r) => {
            let status = r.status();
            let version = format!("{:?}", r.version());
            println!("REPRO\tstream\theaders_s\t{headers_at:.2}\tstatus\t{status}\thttp\t{version}");
            r
        }
        Err(e) => {
            let total = start.elapsed().as_secs_f64();
            println!(
                "REPRO\tstream\tRESULT\tFAIL_SEND\tphase\tawaiting_headers\ttotal_s\t{total:.2}\tis_timeout\t{}\tsource\t{}",
                e.is_timeout(),
                source_chain(&e)
            );
            return;
        }
    };

    let mut stream = resp.bytes_stream();
    let mut first_chunk_s: Option<f64> = None;
    let mut last_chunk_s = headers_at;
    let mut chunks = 0u64;
    let mut bytes = 0u64;
    let mut saw_done = false;

    loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                let now = start.elapsed().as_secs_f64();
                if first_chunk_s.is_none() {
                    first_chunk_s = Some(now);
                }
                last_chunk_s = now;
                chunks += 1;
                bytes += chunk.len() as u64;
                if String::from_utf8_lossy(&chunk).contains("[DONE]") {
                    saw_done = true;
                }
            }
            Some(Err(e)) => {
                let total = start.elapsed().as_secs_f64();
                let phase = if chunks == 0 {
                    "before_first_chunk"
                } else {
                    "mid_stream_between_chunks"
                };
                println!(
                    "REPRO\tstream\tRESULT\tFAIL_BODY\tphase\t{phase}\tfirst_chunk_s\t{:?}\tlast_chunk_s\t{last_chunk_s:.2}\ttotal_s\t{total:.2}\tchunks\t{chunks}\tbytes\t{bytes}\tis_timeout\t{}\tis_body\t{}\tsource\t{}",
                    first_chunk_s,
                    e.is_timeout(),
                    e.is_body(),
                    source_chain(&e)
                );
                return;
            }
            None => {
                let total = start.elapsed().as_secs_f64();
                println!(
                    "REPRO\tstream\tRESULT\tEOF\tfirst_chunk_s\t{:?}\tlast_chunk_s\t{last_chunk_s:.2}\ttotal_s\t{total:.2}\tchunks\t{chunks}\tbytes\t{bytes}\tsaw_done\t{saw_done}",
                    first_chunk_s
                );
                return;
            }
        }
    }
}

// ─── FASE 4 · Canary C: adapter ChatOpenAI con perfil local (post-fix) ───

use crate::agent::events::FnEventHandler;
use crate::llm::types::{LlmRequest, StreamingContext};
use crate::llm::BaseModel;
use crate::messages::{BaseMessage, MessageId};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn ollama_adapter() -> super::ChatOpenAI {
    super::ChatOpenAI::new("ollama", repro_model())
        .with_base_url(OLLAMA_BASE)
        .with_local_cpu_profile()
}

fn canary_request(syspad: usize) -> LlmRequest {
    let mut msgs = Vec::new();
    if syspad > 0 {
        // Nonce único por llamada: evita que Ollama reutilice el KV-cache del
        // prefill (queremos forzar prefill real > 25 s para probar el fix).
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pad = format!("Contexto {nonce} irrelevante para la tarea. ").repeat(syspad);
        msgs.push(BaseMessage::human(pad));
    }
    msgs.push(BaseMessage::human("Respondé únicamente: NEXUM_PROVIDER_OK"));
    LlmRequest::new(msgs).with_max_tokens(64)
}

fn response_text(r: &crate::llm::types::LlmResponse) -> String {
    format!("{:?}", r.message)
}

/// Canary C: el adapter con perfil local resuelve el canary por invoke() y
/// por invoke_streaming() (que degrada a non-stream), < 30 s.
#[tokio::test]
#[ignore]
async fn canary_adapter_local_profile() {
    let adapter = ollama_adapter();
    assert!(!adapter.supports_streaming(), "perfil local ⇒ non-stream");
    assert_eq!(adapter.model_id(), repro_model());

    // invoke() directo
    let t0 = std::time::Instant::now();
    let r = adapter.invoke(canary_request(0)).await.expect("invoke debe pasar");
    let dt = t0.elapsed().as_secs_f64();
    let ok = response_text(&r).contains("NEXUM_PROVIDER_OK");
    println!("CANARY\tC_adapter_invoke\tprovider_id\tollama_local\tmodel_id\t{}\tduration_s\t{dt:.2}\tcanary_ok\t{ok}\tresult\t{}",
        repro_model(), if ok && dt < 30.0 { "PASS" } else { "FAIL" });
    assert!(ok, "respuesta debe contener el canary");
    assert!(dt < 30.0, "duración {dt:.2}s debe ser < 30s");

    // invoke_streaming() debe degradar a non-stream y devolver lo mismo
    let handler = Arc::new(FnEventHandler(|_ev| {}));
    let ctx = StreamingContext {
        event_handler: handler,
        message_id: MessageId::new(),
        cancel: CancellationToken::new(),
    };
    let t1 = std::time::Instant::now();
    let r2 = adapter
        .invoke_streaming(canary_request(0), ctx)
        .await
        .expect("invoke_streaming (degradado) debe pasar");
    let dt2 = t1.elapsed().as_secs_f64();
    let ok2 = response_text(&r2).contains("NEXUM_PROVIDER_OK");
    println!("CANARY\tC_adapter_invoke_streaming_degraded\tduration_s\t{dt2:.2}\tcanary_ok\t{ok2}\tresult\t{}",
        if ok2 && dt2 < 30.0 { "PASS" } else { "FAIL" });
    assert!(ok2 && dt2 < 30.0);
}

/// Verificación del FIX: con prefill largo (syspad) que ANTES cortaba a 25 s,
/// el adapter con read-timeout local ahora completa. Confirma que la causa raíz
/// (read_timeout durante el prefill) queda resuelta para el provider local.
#[tokio::test]
#[ignore]
async fn canary_adapter_survives_long_prefill() {
    let adapter = ollama_adapter();
    let t0 = std::time::Instant::now();
    let r = adapter.invoke(canary_request(8000)).await;
    let dt = t0.elapsed().as_secs_f64();
    match r {
        Ok(resp) => {
            let ok = response_text(&resp).contains("NEXUM_PROVIDER_OK");
            let beyond_old_limit = dt > 25.0;
            println!("CANARY\tC_long_prefill_fixed\tduration_s\t{dt:.2}\tcanary_ok\t{ok}\tbeyond_old_25s_limit\t{beyond_old_limit}\tresult\tPASS\tnote\tno_corta_a_25s");
            // El punto es que el request COMPLETA (antes cortaba a 25 s con
            // is_timeout). No exigimos un tiempo mínimo (el KV-cache/warm puede
            // acelerar); exigimos éxito y respuesta correcta.
            assert!(ok, "el canary debe completar con el fix, sin corte a 25s");
        }
        Err(e) => {
            println!("CANARY\tC_long_prefill_fixed\tduration_s\t{dt:.2}\tresult\tFAIL\terror\t{e}");
            panic!("el fix debe evitar el corte a 25s en prefill local: {e}");
        }
    }
}

/// GATE cancel_test: un StreamingContext cancelado aborta el roundtrip con
/// AgentError::Interrupted (el biased select chequea cancel primero).
#[tokio::test]
#[ignore]
async fn canary_cancel_returns_interrupted() {
    // Adapter con streaming forzado ON para ejercer do_invoke_streaming.
    let adapter = super::ChatOpenAI::new("ollama", repro_model())
        .with_base_url(OLLAMA_BASE)
        .with_streaming(true);
    let handler = Arc::new(FnEventHandler(|_ev| {}));
    let cancel = CancellationToken::new();
    cancel.cancel(); // pre-cancelado
    let ctx = StreamingContext {
        event_handler: handler,
        message_id: MessageId::new(),
        cancel,
    };
    let res = adapter.invoke_streaming(canary_request(0), ctx).await;
    let interrupted = matches!(res, Err(crate::error::AgentError::Interrupted));
    println!("CANARY\tcancel\tinterrupted\t{interrupted}\tresult\t{}", if interrupted { "PASS" } else { "FAIL" });
    assert!(interrupted, "un contexto cancelado debe devolver Interrupted");
}
