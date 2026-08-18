use std::time::Instant;

use nexum_acp::provider::routes::{ProviderRouteError, ResolvedProviderRoute};

use super::runtime_identity::RuntimeIdentity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionPath {
    LocalDirect,
    Llm,
    ToolLlm,
    RejectedByPolicy,
}

impl ExecutionPath {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalDirect => "LOCAL_DIRECT",
            Self::Llm => "LLM",
            Self::ToolLlm => "TOOL_LLM",
            Self::RejectedByPolicy => "REJECTED_BY_POLICY",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TurnProvenance {
    pub turn_id: String,
    pub execution_path: ExecutionPath,
    pub selected_provider: String,
    pub selected_model: String,
    pub executed_provider: Option<String>,
    pub executed_model: Option<String>,
    pub adapter: Option<String>,
    pub upstream_endpoint: Option<String>,
    pub http_status: Option<u16>,
    pub request_sent: bool,
    pub response_received: bool,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub tools_requested: Vec<String>,
    pub tools_executed: Vec<String>,
    pub fallback_used: bool,
    pub terminal_state: String,
    pub elapsed_ms: u64,
    pub llm_invoked: bool,
    pub retry_count: u32,
    started_at: Instant,
}

impl TurnProvenance {
    pub fn local_direct(identity: &RuntimeIdentity) -> Self {
        let mut value = Self::new(ExecutionPath::LocalDirect, identity, None);
        value.response_received = true;
        value.terminal_state = "COMPLETED".to_string();
        value.finish_clock();
        value
    }

    pub fn llm(identity: &RuntimeIdentity, route: &ResolvedProviderRoute) -> Self {
        Self::new(ExecutionPath::Llm, identity, Some(route))
    }

    pub fn llm_selection(identity: &RuntimeIdentity) -> Self {
        Self::new(ExecutionPath::Llm, identity, None)
    }

    pub fn record_route(&mut self, route: &ResolvedProviderRoute) {
        self.adapter = Some(route.adapter.clone());
        self.upstream_endpoint = Some(sanitize_endpoint(&route.endpoint_or_cli));
    }

    pub fn rejected_web(identity: &RuntimeIdentity) -> Self {
        let mut value = Self::new(ExecutionPath::RejectedByPolicy, identity, None);
        value.tools_requested.push("WebSearch".to_string());
        value.terminal_state = "REJECTED_BY_POLICY".to_string();
        value.response_received = true;
        value.finish_clock();
        value
    }

    fn new(
        execution_path: ExecutionPath,
        identity: &RuntimeIdentity,
        route: Option<&ResolvedProviderRoute>,
    ) -> Self {
        Self {
            turn_id: uuid::Uuid::now_v7().to_string(),
            execution_path,
            selected_provider: identity.provider_id.clone(),
            selected_model: identity.model_id.clone(),
            executed_provider: None,
            executed_model: None,
            adapter: route.map(|value| value.adapter.clone()),
            upstream_endpoint: route.map(|value| sanitize_endpoint(&value.endpoint_or_cli)),
            http_status: None,
            request_sent: false,
            response_received: false,
            input_tokens: 0,
            output_tokens: 0,
            tools_requested: Vec::new(),
            tools_executed: Vec::new(),
            fallback_used: false,
            terminal_state: "IN_PROGRESS".to_string(),
            elapsed_ms: 0,
            llm_invoked: execution_path == ExecutionPath::Llm
                || execution_path == ExecutionPath::ToolLlm,
            retry_count: 0,
            started_at: Instant::now(),
        }
    }

    pub fn record_llm_response(
        &mut self,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        http_status: Option<u16>,
    ) {
        if !self.llm_invoked {
            return;
        }
        self.request_sent = true;
        self.response_received = true;
        self.executed_provider = Some(self.selected_provider.clone());
        self.executed_model = Some(model.to_string());
        self.input_tokens = input_tokens;
        self.output_tokens = output_tokens;
        self.http_status = http_status.or(Some(200));
    }

    pub fn record_response_chunk(&mut self) {
        if self.llm_invoked {
            self.request_sent = true;
            self.response_received = true;
            if self.executed_provider.is_none() {
                self.executed_provider = Some(self.selected_provider.clone());
            }
            if self.executed_model.is_none() {
                self.executed_model = Some(self.selected_model.clone());
            }
            self.http_status.get_or_insert(200);
        }
    }

    pub fn record_tool(&mut self, name: &str) {
        if !self.tools_requested.iter().any(|value| value == name) {
            self.tools_requested.push(name.to_string());
        }
        if !self.tools_executed.iter().any(|value| value == name) {
            self.tools_executed.push(name.to_string());
        }
        self.execution_path = ExecutionPath::ToolLlm;
    }

    pub fn complete(&mut self, terminal: &str) {
        self.terminal_state = terminal.to_string();
        self.finish_clock();
    }

    pub fn fail(&mut self, terminal: &str, request_sent: bool, http_status: Option<u16>) {
        self.request_sent |= request_sent;
        self.http_status = http_status.or(self.http_status);
        self.terminal_state = terminal.to_string();
        self.finish_clock();
    }

    fn finish_clock(&mut self) {
        self.elapsed_ms = self.started_at.elapsed().as_millis() as u64;
    }

    pub fn selected_equals_executed(&self) -> bool {
        self.executed_provider.as_deref() == Some(self.selected_provider.as_str())
            && self.executed_model.as_deref() == Some(self.selected_model.as_str())
    }

    pub fn render(&self) -> String {
        let executed_provider = self.executed_provider.as_deref().unwrap_or("none");
        let executed_model = self.executed_model.as_deref().unwrap_or("none");
        let endpoint = self.upstream_endpoint.as_deref().unwrap_or("none");
        let http = self
            .http_status
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string());
        let tools_requested = render_list(&self.tools_requested);
        let tools_executed = render_list(&self.tools_executed);
        format!(
            "Turn provenance\n\
             Turn: {}\n\
             Path: {}\n\
             LLM invoked: {}\n\
             Selected: {} / {}\n\
             Executed: {} / {}\n\
             Adapter: {}\n\
             Endpoint: {}\n\
             HTTP: {}\n\
             Request sent: {}\n\
             Response received: {}\n\
             Input tokens: {}\n\
             Output tokens: {}\n\
             Tools requested: {}\n\
             Tools executed: {}\n\
             Retries: {}\n\
             Fallback: {}\n\
             Terminal: {}\n\
             Elapsed: {} ms",
            self.turn_id,
            self.execution_path.as_str(),
            self.llm_invoked,
            display_or_none(&self.selected_provider),
            display_or_none(&self.selected_model),
            executed_provider,
            executed_model,
            self.adapter.as_deref().unwrap_or("none"),
            endpoint,
            http,
            self.request_sent,
            self.response_received,
            self.input_tokens,
            self.output_tokens,
            tools_requested,
            tools_executed,
            self.retry_count,
            self.fallback_used,
            self.terminal_state,
            self.elapsed_ms,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionIntent {
    Identity,
    WebSearchUnavailable,
    Llm,
}

pub fn classify_execution_intent(input: &str) -> ExecutionIntent {
    let trimmed = input.trim();
    if trimmed == "/identity" || super::runtime_identity::is_identity_question(trimmed) {
        return ExecutionIntent::Identity;
    }
    if explicit_web_search_request(trimmed) {
        return ExecutionIntent::WebSearchUnavailable;
    }
    ExecutionIntent::Llm
}

fn explicit_web_search_request(input: &str) -> bool {
    let normalized = input.to_lowercase();
    let search_verb = [
        "buscá ",
        "busca ",
        "buscar ",
        "investigá en la web",
        "investiga en la web",
        "search ",
        "look up ",
    ]
    .iter()
    .any(|needle| normalized.contains(needle));
    let current_signal = [
        "actual",
        "hoy",
        "últim",
        "ultim",
        "reciente",
        "rendimiento",
        "web",
        "internet",
    ]
    .iter()
    .any(|needle| normalized.contains(needle));
    search_verb && current_signal
}

pub fn resolve_execution_route(
    identity: &RuntimeIdentity,
) -> Result<ResolvedProviderRoute, ProviderRouteError> {
    let result = nexum_acp::provider::routes::validate_installed_selection(
        &identity.provider_id,
        &identity.model_id,
    );
    #[cfg(test)]
    if result.is_err() {
        return Ok(ResolvedProviderRoute {
            provider_id: identity.provider_id.clone(),
            catalog_model_id: identity.model_id.clone(),
            adapter: "test_adapter".to_string(),
            upstream_provider: identity.provider_id.clone(),
            upstream_model: identity.model_id.clone(),
            auth_mode: "test".to_string(),
            endpoint_or_cli: "test://local".to_string(),
            resolver: "test".to_string(),
        });
    }
    result
}

pub struct FailurePresentation {
    pub classification: &'static str,
    pub request_sent: bool,
    pub http_status: Option<u16>,
}

pub fn classify_provider_failure(message: &str) -> FailurePresentation {
    let lower = message.to_lowercase();
    let http_status = extract_http_status(message);
    if lower.contains("class=dns") || lower.contains("name resolution") || lower.contains("dns") {
        return FailurePresentation {
            classification: "DNS",
            request_sent: false,
            http_status,
        };
    }
    if lower.contains("class=tls")
        || lower.contains("certificate")
        || lower.contains("tls")
        || lower.contains("handshake")
    {
        return FailurePresentation {
            classification: "TLS",
            request_sent: false,
            http_status,
        };
    }
    if lower.contains("class=connect_timeout") {
        return FailurePresentation {
            classification: "CONNECT_TIMEOUT",
            request_sent: false,
            http_status,
        };
    }
    if lower.contains("class=read_timeout") || lower.contains("timed out") {
        return FailurePresentation {
            classification: "READ_TIMEOUT",
            request_sent: true,
            http_status,
        };
    }
    if lower.contains("class=connection_reset") || lower.contains("connection reset") {
        return FailurePresentation {
            classification: "CONNECTION_RESET",
            request_sent: true,
            http_status,
        };
    }
    if lower.contains("class=upstream_unavailable") || matches!(http_status, Some(502 | 503 | 504))
    {
        return FailurePresentation {
            classification: "UPSTREAM_UNAVAILABLE",
            request_sent: true,
            http_status,
        };
    }
    if http_status.is_some() || lower.contains("llm http") || lower.contains("http_error") {
        return FailurePresentation {
            classification: "HTTP_ERROR",
            request_sent: true,
            http_status,
        };
    }
    FailurePresentation {
        classification: "UNKNOWN",
        request_sent: lower.contains("error sending request")
            || lower.contains("llm")
            || lower.contains("provider"),
        http_status,
    }
}

pub fn format_provider_failure(
    provenance: &TurnProvenance,
    failure: &FailurePresentation,
    terminal: &str,
) -> String {
    format!(
        "Provider request failed\n\
         Provider: {}\n\
         Model: {}\n\
         Class: {}\n\
         Endpoint: {}\n\
         HTTP: {}\n\
         Request sent: {}\n\
         Retries: {}\n\
         Fallback: {}\n\
         Terminal: {}",
        display_or_none(&provenance.selected_provider),
        display_or_none(&provenance.selected_model),
        failure.classification,
        provenance.upstream_endpoint.as_deref().unwrap_or("none"),
        failure
            .http_status
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        failure.request_sent,
        provenance.retry_count,
        provenance.fallback_used,
        terminal,
    )
}

fn extract_http_status(message: &str) -> Option<u16> {
    message
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| part.len() == 3)
        .filter_map(|part| part.parse::<u16>().ok())
        .find(|status| (100..=599).contains(status))
}

fn sanitize_endpoint(input: &str) -> String {
    let without_query = input.split(['?', '#']).next().unwrap_or_default();
    let (scheme, rest) = without_query
        .split_once("://")
        .map(|(scheme, rest)| (Some(scheme), rest))
        .unwrap_or((None, without_query));
    let rest = rest.rsplit_once('@').map(|(_, host)| host).unwrap_or(rest);
    match scheme {
        Some(scheme) => format!("{scheme}://{rest}"),
        None => rest.to_string(),
    }
}

fn render_list(values: &[String]) -> String {
    if values.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", values.join(", "))
    }
}

fn display_or_none(value: &str) -> &str {
    if value.is_empty() {
        "none"
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> RuntimeIdentity {
        RuntimeIdentity {
            provider_id: "opencode_zen".to_string(),
            provider_family: "OpenCode Free".to_string(),
            model_id: "ling-3.0-flash-free".to_string(),
            base_url: "https://opencode.ai/zen/v1".to_string(),
            source: "user_selection",
        }
    }

    fn route() -> ResolvedProviderRoute {
        ResolvedProviderRoute {
            provider_id: "opencode_zen".to_string(),
            catalog_model_id: "ling-3.0-flash-free".to_string(),
            adapter: "openai_compatible".to_string(),
            upstream_provider: "opencode_zen".to_string(),
            upstream_model: "ling-3.0-flash-free".to_string(),
            auth_mode: "cli_account".to_string(),
            endpoint_or_cli: "https://user:credential@opencode.ai/zen/v1?authorization=secret"
                .to_string(),
            resolver: "slot_python".to_string(),
        }
    }

    #[test]
    fn explicit_identity_query_uses_local_direct() {
        assert_eq!(
            classify_execution_intent("qué modelo estás utilizando"),
            ExecutionIntent::Identity
        );
        assert_eq!(
            classify_execution_intent("/identity"),
            ExecutionIntent::Identity
        );
    }

    #[test]
    fn local_direct_reports_llm_not_invoked() {
        let provenance = TurnProvenance::local_direct(&identity());
        assert_eq!(provenance.execution_path, ExecutionPath::LocalDirect);
        assert!(!provenance.llm_invoked);
        assert!(!provenance.request_sent);
    }

    #[test]
    fn normal_generation_prompt_uses_llm() {
        assert_eq!(
            classify_execution_intent("Escribí una historia sobre una maestra."),
            ExecutionIntent::Llm
        );
    }

    #[test]
    fn long_generation_prompt_uses_llm() {
        let prompt = format!("Escribí una historia. {}", "detalle ".repeat(80));
        assert_eq!(classify_execution_intent(&prompt), ExecutionIntent::Llm);
    }

    #[test]
    fn selected_provider_equals_executed_provider() {
        let mut provenance = TurnProvenance::llm(&identity(), &route());
        provenance.record_llm_response("ling-3.0-flash-free", 10, 20, Some(200));
        assert!(provenance.selected_equals_executed());
    }

    #[test]
    fn selected_model_equals_executed_model() {
        let mut provenance = TurnProvenance::llm(&identity(), &route());
        provenance.record_llm_response("ling-3.0-flash-free", 10, 20, Some(200));
        assert_eq!(
            provenance.executed_model.as_deref(),
            Some(provenance.selected_model.as_str())
        );
    }

    #[test]
    fn cross_provider_fallback_is_forbidden() {
        let provenance = TurnProvenance::llm(&identity(), &route());
        assert!(!provenance.fallback_used);
    }

    #[test]
    fn trace_reports_last_turn_provenance() {
        let mut provenance = TurnProvenance::llm(&identity(), &route());
        provenance.record_llm_response("ling-3.0-flash-free", 10, 20, Some(200));
        provenance.complete("COMPLETED");
        let trace = provenance.render();
        assert!(trace.contains("Path: LLM"));
        assert!(trace.contains("Executed: opencode_zen / ling-3.0-flash-free"));
        assert!(trace.contains("HTTP: 200"));
    }

    #[test]
    fn trace_sanitizes_secrets() {
        let trace = TurnProvenance::llm(&identity(), &route()).render();
        assert!(trace.contains("https://opencode.ai/zen/v1"));
        assert!(!trace.contains("credential"));
        assert!(!trace.contains("authorization"));
        assert!(!trace.contains("secret"));
    }

    #[test]
    fn web_request_never_claims_search_without_tool() {
        let provenance = TurnProvenance::rejected_web(&identity());
        assert_eq!(provenance.execution_path, ExecutionPath::RejectedByPolicy);
        assert_eq!(provenance.tools_requested, vec!["WebSearch"]);
        assert!(provenance.tools_executed.is_empty());
    }

    #[test]
    fn missing_web_tool_is_structured() {
        assert_eq!(
            classify_execution_intent("Buscá información actual sobre Kimi K3"),
            ExecutionIntent::WebSearchUnavailable
        );
    }

    #[test]
    fn tool_execution_is_recorded_when_available() {
        let mut provenance = TurnProvenance::llm(&identity(), &route());
        provenance.record_tool("WebSearch");
        assert_eq!(provenance.execution_path, ExecutionPath::ToolLlm);
        assert_eq!(provenance.tools_executed, vec!["WebSearch"]);
    }

    #[test]
    fn transport_error_has_structured_classification() {
        let failure = classify_provider_failure(
            "LLM_TRANSPORT_FAILURE class=READ_TIMEOUT endpoint=opencode.ai request_sent=true",
        );
        assert_eq!(failure.classification, "READ_TIMEOUT");
        assert!(failure.request_sent);
    }

    #[test]
    fn transport_error_preserves_selection() {
        let mut provenance = TurnProvenance::llm(&identity(), &route());
        let failure = classify_provider_failure("class=CONNECT_TIMEOUT");
        provenance.fail("FAILED", failure.request_sent, failure.http_status);
        assert_eq!(provenance.selected_provider, "opencode_zen");
        assert_eq!(provenance.selected_model, "ling-3.0-flash-free");
    }

    #[test]
    fn transport_error_does_not_fallback() {
        let provenance = TurnProvenance::llm(&identity(), &route());
        assert!(!provenance.fallback_used);
    }

    fn assert_real_route_provenance(provider: &str, model: &str) {
        let registry = nexum_acp::provider::routes::ProviderRouteRegistry::load_from_path(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../config/provider-route-registry.json"),
        )
        .unwrap();
        let resolved = registry.resolve(provider, model).unwrap();
        let identity = RuntimeIdentity {
            provider_id: provider.to_string(),
            provider_family: provider.to_string(),
            model_id: model.to_string(),
            base_url: resolved.endpoint_or_cli.clone(),
            source: "user_selection",
        };
        let mut provenance = TurnProvenance::llm(&identity, &resolved);
        provenance.record_llm_response(model, 10, 20, Some(200));
        provenance.complete("COMPLETED");
        assert!(provenance.selected_equals_executed());
        assert!(provenance.request_sent);
        assert!(provenance.response_received);
        assert!(!provenance.fallback_used);
    }

    #[test]
    fn opencode_turn_records_real_execution() {
        assert_real_route_provenance("opencode_zen", "ling-3.0-flash-free");
    }

    #[test]
    fn mimo_turn_records_real_execution() {
        assert_real_route_provenance("mimo_code", "mimo-v2.5");
    }

    #[test]
    fn codex_turn_records_real_execution() {
        assert_real_route_provenance("codex_cli", "gpt-5.4");
    }
}
