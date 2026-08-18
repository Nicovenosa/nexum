//! executor.rs 单元测试。
//!
//! 重点覆盖 [`intercept_immediate_command`]——命令拦截是 execute_prompt 的
//! 前置短路逻辑，任何回归（如忘记 `push_done`）都会导致 TUI 永久 loading
//! （issue_2026-05-29-immediate-command-missing-push-done）。
//!
//! Mock 命名遵循 CLAUDE.md：`make_` 前缀（函数），`Mock` 前缀（结构体）。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nexum_agent::{
    agent::{events::AgentEvent as ExecutorEvent, AgentCancellationToken},
    messages::{BaseMessage, MessageContent},
};

use super::{
    intercept_immediate_command, is_local_micro_e2e_envelope, InterceptRequest, PromptStopReason,
};
use crate::{
    provider::NexumConfig,
    session::event_sink::EventSink,
    task::{
        EvidencePolicy, ExecutionBudgetV1, OutputFormat, TaskEnvelopeV1, TaskEnvelopeVersion,
        TaskPriority, TaskRisk, TaskSource,
    },
};

// ── Mock EventSink ─────────────────────────────────────────────────────────

/// Mock EventSink，记录所有 push_done 调用。
struct MockEventSink {
    push_done_count: Mutex<usize>,
    pushed_events: Mutex<Vec<String>>,
}

impl MockEventSink {
    fn new() -> Self {
        Self {
            push_done_count: Mutex::new(0),
            pushed_events: Mutex::new(Vec::new()),
        }
    }

    fn push_done_count(&self) -> usize {
        *self.push_done_count.lock().unwrap()
    }
}

#[async_trait]
impl EventSink for MockEventSink {
    async fn push_event(&self, _session_id: &str, event: &ExecutorEvent, _context_window: u32) {
        let json = serde_json::to_string(event).unwrap_or_default();
        self.pushed_events.lock().unwrap().push(json);
    }

    async fn push_done(&self, _session_id: &str) {
        *self.push_done_count.lock().unwrap() += 1;
    }
}

// ── Helper 工厂函数 ─────────────────────────────────────────────────────────

/// 构造最小 InterceptRequest（auxiliary_model / thread_store 等均为 None）。
///
/// 8 个参数全部是测试所需的引用——测试构造函数不强制参数对象化。
#[allow(clippy::too_many_arguments)]
fn make_intercept_request<'a>(
    content: &'a MessageContent,
    history: &'a [BaseMessage],
    session_id: &'a str,
    cancel: &'a AgentCancellationToken,
    nexum_config: &'a Arc<NexumConfig>,
    event_sink: &'a Arc<dyn EventSink>,
    bg_event_tx: &'a tokio::sync::mpsc::UnboundedSender<ExecutorEvent>,
    bg_registry: &'a Arc<nexum_middlewares::subagent::BackgroundTaskRegistry>,
) -> InterceptRequest<'a> {
    InterceptRequest {
        content,
        history,
        cwd: "/tmp",
        session_id,
        cancel,
        nexum_config,
        event_sink,
        auxiliary_model: &None,
        thread_store: None,
        thread_id: None,
        bg_event_tx,
        bg_registry,
        frozen: None,
    }
}

/// 构造共享的 bg registry + bg channel（拦截测试不实际触发 bg，但需要传入句柄）。
fn make_bg_infra() -> (
    tokio::sync::mpsc::UnboundedSender<ExecutorEvent>,
    Arc<nexum_middlewares::subagent::BackgroundTaskRegistry>,
) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<ExecutorEvent>();
    let (notif_tx, _notif_rx) = tokio::sync::mpsc::unbounded_channel();
    let registry = Arc::new(nexum_middlewares::subagent::BackgroundTaskRegistry::new(
        notif_tx,
    ));
    (tx, registry)
}

fn make_local_micro_envelope() -> TaskEnvelopeV1 {
    TaskEnvelopeV1 {
        version: TaskEnvelopeVersion::V1,
        envelope_id: "host-e2e".to_string(),
        source: TaskSource::Voice,
        objective: "Respondé únicamente con: NEXUM_ACP_E2E_OK".to_string(),
        user_input: "Respondé únicamente con: NEXUM_ACP_E2E_OK".to_string(),
        session_id: "voice-session".to_string(),
        thread_id: "voice-thread".to_string(),
        workspace: Some(".".to_string()),
        constraints: Vec::new(),
        allowed_tools: Vec::new(),
        evidence_refs: Vec::new(),
        success_criteria: Vec::new(),
        output_format: OutputFormat::Text,
        execution_budget: ExecutionBudgetV1::default(),
        evidence_policy: EvidencePolicy {
            require_evidence: false,
            minimum_evidence_refs: 0,
            allow_unverified_output: true,
        },
        priority: TaskPriority::Normal,
        risk: TaskRisk::Low,
        sanitized_metadata: Default::default(),
    }
}

#[test]
fn test_local_micro_solo_acepta_el_envelope_e2e_explicito_sin_tools() {
    let envelope = make_local_micro_envelope();
    assert!(is_local_micro_e2e_envelope(&envelope));

    let mut with_tool = envelope.clone();
    with_tool.allowed_tools.push("Read".to_string());
    assert!(!is_local_micro_e2e_envelope(&with_tool));

    let mut other_objective = envelope.clone();
    other_objective.objective = "Decime la hora".to_string();
    assert!(!is_local_micro_e2e_envelope(&other_objective));

    let mut markdown = envelope;
    markdown.output_format = OutputFormat::Markdown;
    assert!(!is_local_micro_e2e_envelope(&markdown));
}

// ── intercept_immediate_command: 路径分支测试 ─────────────────────────────

/// 普通 slash 命令（非 Immediate 注册）：不在默认注册表中 → 返回 None
#[tokio::test]
async fn test_intercept_unknown_command_returns_none() {
    // Arrange
    let content = MessageContent::text("/nonexistent");
    let history: Vec<BaseMessage> = vec![];
    let cancel = AgentCancellationToken::new();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let sink: Arc<dyn EventSink> = Arc::new(MockEventSink::new());
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    let result = intercept_immediate_command(req).await;

    // Assert：未知命令不拦截，继续走 agent 管线
    assert!(result.is_none(), "未知命令应返回 None 继续走 agent 管线");
}

/// 普通文本（无 `/` 前缀）：返回 None
#[tokio::test]
async fn test_intercept_plain_text_returns_none() {
    // Arrange
    let content = MessageContent::text("你好，请帮我写代码");
    let history: Vec<BaseMessage> = vec![];
    let cancel = AgentCancellationToken::new();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let sink: Arc<dyn EventSink> = Arc::new(MockEventSink::new());
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    let result = intercept_immediate_command(req).await;

    // Assert：普通文本不拦截
    assert!(result.is_none(), "普通文本应返回 None");
}

/// 单个 `/` 字符：strip 后为空 → 返回 None
#[tokio::test]
async fn test_intercept_slash_only_returns_none() {
    // Arrange
    let content = MessageContent::text("/");
    let history: Vec<BaseMessage> = vec![];
    let cancel = AgentCancellationToken::new();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let sink: Arc<dyn EventSink> = Arc::new(MockEventSink::new());
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    let result = intercept_immediate_command(req).await;

    // Assert：单个 `/` 应返回 None（不空命中命令）
    assert!(result.is_none(), "单个 `/` 应返回 None");
}

// ── intercept_immediate_command: Immediate 命令拦截（/clear） ─────────────

/// `/clear` 是 Immediate 命令：拦截成功，返回 Some(PromptResult)
#[tokio::test]
async fn test_intercept_clear_command_returns_some() {
    // Arrange
    let content = MessageContent::text("/clear");
    let history: Vec<BaseMessage> = vec![BaseMessage::human("你好"), BaseMessage::ai("世界")];
    let cancel = AgentCancellationToken::new();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let sink: Arc<dyn EventSink> = Arc::new(MockEventSink::new());
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    let result = intercept_immediate_command(req).await;

    // Assert：clear 命令应被拦截
    assert!(result.is_some(), "/clear 应返回 Some(PromptResult)");
    let prompt_result = result.unwrap();
    assert!(prompt_result.ok, "拦截结果 ok 应为 true");
    assert_eq!(
        prompt_result.stop_reason,
        PromptStopReason::EndTurn,
        "clear 命令停止原因应为 EndTurn"
    );
    assert!(
        prompt_result.messages.is_empty(),
        "clear 命令应清空历史，messages 为空"
    );
}

/// `/clear` 别名 `/cls` 也应被拦截
#[tokio::test]
async fn test_intercept_clear_alias_cls_returns_some() {
    // Arrange
    let content = MessageContent::text("/cls");
    let history: Vec<BaseMessage> = vec![BaseMessage::human("历史消息")];
    let cancel = AgentCancellationToken::new();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let sink: Arc<dyn EventSink> = Arc::new(MockEventSink::new());
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    let result = intercept_immediate_command(req).await;

    // Assert：cls 别名也应被拦截
    assert!(result.is_some(), "/cls 别名应返回 Some(PromptResult)");
    let prompt_result = result.unwrap();
    assert!(
        prompt_result.messages.is_empty(),
        "/cls 应清空历史，messages 为空"
    );
}

/// `/reset` 别名也应被拦截（ClearCommand 的第二个别名）
#[tokio::test]
async fn test_intercept_clear_alias_reset_returns_some() {
    // Arrange
    let content = MessageContent::text("/reset");
    let history: Vec<BaseMessage> = vec![BaseMessage::ai("对话历史")];
    let cancel = AgentCancellationToken::new();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let sink: Arc<dyn EventSink> = Arc::new(MockEventSink::new());
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    let result = intercept_immediate_command(req).await;

    // Assert
    assert!(result.is_some(), "/reset 别名应返回 Some(PromptResult)");
}

// ── intercept_immediate_command: push_done TRAP 验证 ──────────────────────

/// [TRAP] Immediate 命令拦截后必须调用 `push_done`，否则 TUI 永久 loading
/// （issue_2026-05-29-immediate-command-missing-push-done）
#[tokio::test]
async fn test_intercept_clear_command_calls_push_done() {
    // Arrange
    let content = MessageContent::text("/clear");
    let history: Vec<BaseMessage> = vec![];
    let cancel = AgentCancellationToken::new();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let mock_sink = Arc::new(MockEventSink::new());
    let sink: Arc<dyn EventSink> = Arc::clone(&mock_sink) as Arc<dyn EventSink>;
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    intercept_immediate_command(req).await;

    // Assert：必须调用 push_done 一次
    assert_eq!(
        mock_sink.push_done_count(),
        1,
        "Immediate 命令拦截后必须调用 push_done（TRAP: TUI 永久 loading）"
    );
}

/// 未拦截路径不应调用 push_done（push_done 由后续 pump 负责）
#[tokio::test]
async fn test_intercept_no_match_does_not_call_push_done() {
    // Arrange
    let content = MessageContent::text("普通文本");
    let history: Vec<BaseMessage> = vec![];
    let cancel = AgentCancellationToken::new();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let mock_sink = Arc::new(MockEventSink::new());
    let sink: Arc<dyn EventSink> = Arc::clone(&mock_sink) as Arc<dyn EventSink>;
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    intercept_immediate_command(req).await;

    // Assert：未拦截时 push_done 为 0（由后续 pump 负责）
    assert_eq!(
        mock_sink.push_done_count(),
        0,
        "未拦截路径不应调用 push_done"
    );
}

// ── intercept_immediate_command: cancel 路径验证 ──────────────────────────

/// cancel 信号已触发时：intercept 仍返回 Some（已拦截），且必然调用 push_done。
///
/// 注意：tokio::select! 对已 ready 的 cancel 和快速完成的命令执行是竞速关系，
/// 对瞬时命令（如 /clear）执行分支可能先完成。本测试只验证不变量：
/// 无论哪个分支执行，push_done 都被调用、结果非 None。
#[tokio::test]
async fn test_intercept_with_cancelled_token_still_returns_some() {
    // Arrange
    let content = MessageContent::text("/clear");
    let history: Vec<BaseMessage> = vec![BaseMessage::human("hello"), BaseMessage::ai("world")];
    let cancel = AgentCancellationToken::new();
    // 预先 cancel，与命令执行竞速
    cancel.cancel();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let mock_sink = Arc::new(MockEventSink::new());
    let sink: Arc<dyn EventSink> = Arc::clone(&mock_sink) as Arc<dyn EventSink>;
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    let result = intercept_immediate_command(req).await;

    // Assert：无论 select 走哪个分支，结果都应非 None（命令已拦截或被取消）
    assert!(result.is_some(), "已 cancel 的拦截路径仍应返回 Some");
    // 不变量：push_done 必被调用（TRAP 守护）
    assert!(
        mock_sink.push_done_count() >= 1,
        "无论 cancel 还是执行分支，push_done 必被调用至少一次"
    );
}

// ── intercept_immediate_command: recall_items 验证 ─────────────────────────

/// Immediate 命令拦截：recall_items 必须为空（命令不产生 recall）
#[tokio::test]
async fn test_intercept_immediate_returns_empty_recall_items() {
    // Arrange
    let content = MessageContent::text("/clear");
    let history: Vec<BaseMessage> = vec![];
    let cancel = AgentCancellationToken::new();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let sink: Arc<dyn EventSink> = Arc::new(MockEventSink::new());
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    let result = intercept_immediate_command(req).await;

    // Assert：recall_items 必须为空
    let prompt_result = result.unwrap();
    assert!(
        prompt_result.recall_items.is_empty(),
        "Immediate 命令不应产生 recall items"
    );
}

// ── intercept_immediate_command: ok 字段恒为 true 验证 ────────────────────

/// Immediate 命令拦截：ok 字段恒为 true（命令成功 = agent 不构建 = ok）
#[tokio::test]
async fn test_intercept_immediate_ok_always_true() {
    // Arrange
    let content = MessageContent::text("/clear");
    let history: Vec<BaseMessage> = vec![];
    let cancel = AgentCancellationToken::new();
    let nexum_config: Arc<NexumConfig> = Arc::new(Default::default());
    let sink: Arc<dyn EventSink> = Arc::new(MockEventSink::new());
    let (bg_tx, bg_reg) = make_bg_infra();
    let req = make_intercept_request(
        &content,
        &history,
        "test-session",
        &cancel,
        &nexum_config,
        &sink,
        &bg_tx,
        &bg_reg,
    );

    // Act
    let result = intercept_immediate_command(req).await;

    // Assert
    let prompt_result = result.unwrap();
    assert!(
        prompt_result.ok,
        "Immediate 命令拦截后 ok 必须为 true（命令成功 = agent 不构建）"
    );
}

// ─── Mensaje de tope agotado ──────────────────────────────────────────────────
//
// Un tope sin mensaje es el mismo cuelgue, más corto. Estos tests assertan que
// el usuario se entera de que el turno se cortó, en su idioma, y con lo que el
// turno sí llegó a conseguir.

use super::mensaje_tope_agotado;
use nexum_agent::messages::ToolCallRequest;

fn tc(nombre: &str) -> ToolCallRequest {
    ToolCallRequest::new(format!("id-{nombre}"), nombre, serde_json::json!({}))
}

#[test]
fn tope_agotado_dice_el_limite_en_castellano() {
    let msg = mensaje_tope_agotado(15, &[]);
    assert!(
        msg.contains("15"),
        "el mensaje debe decir contra qué tope chocó: {msg}"
    );
    assert!(
        !msg.contains("Max iterations exceeded"),
        "no debe filtrarse el Display del error en inglés: {msg}"
    );
    assert!(
        msg.contains("Corté el turno"),
        "el usuario tiene que entender que el turno se cortó: {msg}"
    );
}

#[test]
fn tope_agotado_enumera_las_herramientas_que_si_corrio() {
    let mensajes = vec![
        BaseMessage::ai_with_tool_calls("", vec![tc("read_file")]),
        BaseMessage::ai_with_tool_calls("", vec![tc("bash"), tc("read_file")]),
    ];
    let msg = mensaje_tope_agotado(15, &mensajes);
    assert!(msg.contains("read_file"), "falta read_file: {msg}");
    assert!(msg.contains("bash"), "falta bash: {msg}");
    assert!(
        msg.contains("read_file ×2"),
        "debe contar las repeticiones: {msg}"
    );
    assert!(
        msg.contains("3 llamadas"),
        "debe reportar el total de llamadas: {msg}"
    );
}

#[test]
fn tope_agotado_conserva_la_media_respuesta_que_ya_habia() {
    let mensajes = vec![
        BaseMessage::ai("primer intento"),
        BaseMessage::ai_with_tool_calls("", vec![tc("bash")]),
        BaseMessage::ai("encontré el archivo en src/main.rs"),
    ];
    let msg = mensaje_tope_agotado(15, &mensajes);
    assert!(
        msg.contains("encontré el archivo en src/main.rs"),
        "el texto parcial no se tira: es lo que el turno sí produjo: {msg}"
    );
    assert!(
        !msg.contains("primer intento"),
        "sólo el último texto, no el historial entero: {msg}"
    );
}

#[test]
fn tope_agotado_sin_herramientas_lo_dice_explicitamente() {
    let msg = mensaje_tope_agotado(15, &[BaseMessage::ai("dando vueltas")]);
    assert!(
        msg.contains("No llegué a usar ninguna herramienta"),
        "el caso vacío se nombra, no se omite: {msg}"
    );
}

#[test]
fn tope_agotado_trunca_el_parcial_largo_sin_cortar_el_resto() {
    let largo = "x".repeat(5000);
    let msg = mensaje_tope_agotado(15, &[BaseMessage::ai(largo)]);
    assert!(msg.contains('…'), "el parcial largo se trunca: {msg}");
    assert!(
        msg.contains("Pedime que siga desde acá"),
        "el cierre accionable sobrevive a la truncación"
    );
    assert!(msg.len() < 1200, "el mensaje no puede inundar el chat");
}

// ─── Modelo sin soporte de herramientas ───────────────────────────────────────

#[test]
fn mensaje_modelo_sin_tools_nombra_el_modelo_y_la_salida() {
    let msg = super::mensaje_modelo_sin_tools("moondream:latest");
    assert!(msg.contains("moondream:latest"), "debe nombrar el modelo: {msg}");
    assert!(
        msg.contains("/modelo"),
        "el usuario tiene que saber cómo salir del problema: {msg}"
    );
    assert!(
        msg.contains("catálogo"),
        "de dónde sale el dato importa: no es una adivinanza del runtime: {msg}"
    );
}

#[test]
fn mensaje_modelo_sin_tools_no_culpa_al_modelo_de_haber_fallado() {
    // No falló: no lo intenta. La distinción evita que el usuario reintente
    // esperando otro resultado.
    let msg = super::mensaje_modelo_sin_tools("moondream:latest");
    assert!(msg.contains("no lo intenta"), "{msg}");
}
