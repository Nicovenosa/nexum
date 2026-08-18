//! TokenUsage → langfuse_usage_details 转换 + LLM 重试 metadata 组装。
//!
//! 纯函数无状态。提取自原 tracer.rs::on_llm_end（383-425 行）。

use std::collections::HashMap;

use nexum_agent::llm::types::TokenUsage;

use super::context::RetryAttempt;

/// 将 TokenUsage 转换为 Langfuse usage_details HashMap。
///
/// [TRAP] input_tokens 已被适配器规范化（Anthropic: raw + cache_creation + cache_read），
/// Langfuse 要求 input 为不含缓存的原始值，需减去缓存部分。**禁止简化此减法**，
/// 否则 Langfuse token 统计会翻倍。
pub(crate) fn build_usage_details(usage: &TokenUsage) -> HashMap<String, i32> {
    let mut map = HashMap::new();
    let cache_creation = usage.cache_creation_input_tokens.unwrap_or(0);
    let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
    // input_tokens 已被适配器规范化（Anthropic: raw + cache_creation + cache_read），
    // Langfuse 要求 input 为不含缓存的原始值，需减去缓存部分。
    let raw_input = usage
        .input_tokens
        .saturating_sub(cache_creation + cache_read);
    let total = raw_input + usage.output_tokens + cache_creation + cache_read;
    map.insert("input".to_string(), raw_input as i32);
    map.insert("output".to_string(), usage.output_tokens as i32);
    map.insert("total".to_string(), total as i32);
    if cache_creation > 0 {
        map.insert(
            "cache_creation_input_tokens".to_string(),
            cache_creation as i32,
        );
    }
    if cache_read > 0 {
        map.insert("cache_read_input_tokens".to_string(), cache_read as i32);
    }
    map
}

/// 组装 LLM 重试 metadata（retry_count + retries 数组），无重试时返回 None。
pub(crate) fn build_retry_metadata(retries: &[RetryAttempt]) -> Option<serde_json::Value> {
    if retries.is_empty() {
        return None;
    }
    let retry_values: Vec<serde_json::Value> = retries
        .iter()
        .map(|r| {
            serde_json::json!({
                "attempt": r.attempt,
                "max_attempts": r.max_attempts,
                "delay_ms": r.delay_ms,
                "error": r.error,
            })
        })
        .collect();
    Some(serde_json::json!({
        "retry_count": retries.len(),
        "retries": retry_values,
    }))
}
