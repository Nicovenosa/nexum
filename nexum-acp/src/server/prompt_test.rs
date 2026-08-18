use super::*;
use serde_json::json;

#[test]
fn test_parse_task_envelope_accepts_optional_task_envelope_without_history() {
    let params = json!({
        "taskEnvelope": {
            "version": "v1",
            "envelope_id": "env-1",
            "source": "api",
            "objective": "validar contrato",
            "user_input": "entrada",
            "session_id": "session-1",
            "thread_id": "thread-1",
            "workspace": null,
            "constraints": [],
            "allowed_tools": ["Read"],
            "evidence_refs": [],
            "success_criteria": ["ok"],
            "output_format": "json",
            "execution_budget": {},
            "evidence_policy": {
                "require_evidence": false,
                "minimum_evidence_refs": 0,
                "allow_unverified_output": true
            },
            "priority": "normal",
            "risk": "low",
            "sanitized_metadata": {}
        }
    });

    let envelope = parse_task_envelope(&params).unwrap().unwrap();

    assert_eq!(envelope.envelope_id, "env-1");
    assert_eq!(envelope.source, nexum_acp::task::TaskSource::Api);
}

#[test]
fn test_parse_task_envelope_keeps_existing_prompt_payload_compatible() {
    let envelope = parse_task_envelope(&json!({"sessionId": "session-1"})).unwrap();

    assert!(
        envelope.is_none(),
        "sin taskEnvelope el prompt existente debe seguir funcionando"
    );
}

#[test]
fn test_voice_envelope_llega_al_input_que_alimenta_prompt_execution_context() {
    let params = json!({
        "message": {"role": "user", "content": "analizá"},
        "taskEnvelope": {
            "version": "v1", "envelope_id": "voice-1", "source": "voice",
            "objective": "analizar", "user_input": "analizá", "session_id": "s",
            "thread_id": "t", "workspace": null, "constraints": [], "allowed_tools": [],
            "evidence_refs": [], "success_criteria": [], "output_format": "text",
            "execution_budget": {},
            "evidence_policy": {"require_evidence": false, "minimum_evidence_refs": 0, "allow_unverified_output": true},
            "priority": "normal", "risk": "low", "sanitized_metadata": {}
        }
    });
    let input = parse_prompt_execution_input(&params).unwrap();
    assert_eq!(
        input.task_envelope.unwrap().source,
        nexum_acp::task::TaskSource::Voice
    );
}

#[test]
fn missing_stable_envelope_is_marked_for_fail_closed_execution() {
    let input = parse_prompt_execution_input(&json!({
        "message": {"role": "user", "content": "Hola"},
        "stableProfile": true
    }))
    .unwrap();
    assert!(input.stable_profile);
    assert!(input.task_envelope.is_none());
}

/// 测试 strip_leaked_prepends：有原始历史时，通过 ID 匹配定位并剥离 leaked system prepends
#[test]
fn test_strip_leaked_prepends_有历史时剥离头部system消息() {
    // Arrange: 原始历史 [Human("hello"), Ai("hi")]
    let history = [BaseMessage::human("hello"), BaseMessage::ai("hi")];
    // 模拟 execute() 错误路径返回的 messages:
    // [SystemPrepend, SystemPrompt, Human("hello"), Ai("hi"), Human("new"), Ai("response")]
    let leaked_system_1 = BaseMessage::system("injected by middleware");
    let leaked_system_2 = BaseMessage::system("system prompt");
    let result_messages = vec![
        leaked_system_1,
        leaked_system_2,
        history[0].clone(),
        history[1].clone(),
        BaseMessage::human("new question"),
        BaseMessage::ai("response"),
    ];
    // Act
    let cleaned = strip_leaked_prepends(&result_messages, history.first().map(|m| m.id()));
    // Assert: 应该去掉头部两条 leaked system，保留从原始历史开始的所有消息
    assert_eq!(cleaned.len(), 4, "应去掉2条leaked system，剩4条");
    assert_eq!(
        cleaned[0].id(),
        history[0].id(),
        "第一条应为原始历史的第一条"
    );
    assert!(!cleaned[0].is_system(), "不应包含leaked system");
}

/// 测试 strip_leaked_prepends：原始历史为空时，剥离所有头部 system 消息
#[test]
fn test_strip_leaked_prepends_空历史时剥离头部system() {
    // Arrange: 空历史
    let history: Vec<BaseMessage> = vec![];
    let result_messages = vec![
        BaseMessage::system("injected by middleware"),
        BaseMessage::system("system prompt"),
        BaseMessage::human("new question"),
        BaseMessage::ai("response"),
    ];
    // Act
    let cleaned = strip_leaked_prepends(&result_messages, history.first().map(|m| m.id()));
    // Assert: 应该去掉头部两条 system，只保留 human + ai
    assert_eq!(cleaned.len(), 2, "应去掉2条leaked system，剩2条");
    assert!(!cleaned[0].is_system(), "第一条不应是system消息");
}

/// 测试 strip_leaked_prepends：原始历史在 result 中找不到（compact 替换场景）
#[test]
fn test_strip_leaked_prepends_历史id找不到时原样返回() {
    // Arrange: 原始历史有一条消息
    let history = [BaseMessage::human("hello")];
    // result_messages 中不包含原始历史的消息（compact 替换了所有消息）
    let result_messages = vec![
        BaseMessage::system("system prompt"),
        BaseMessage::human("compacted summary"),
        BaseMessage::ai("response"),
    ];
    // Act
    let cleaned = strip_leaked_prepends(&result_messages, history.first().map(|m| m.id()));
    // Assert: 找不到原始历史，原样返回
    assert_eq!(cleaned.len(), 3, "找不到原始历史时应原样返回");
}

/// 测试 strip_leaked_prepends：没有 leaked prepends 时正常返回
#[test]
fn test_strip_leaked_prepends_无leaked时正常返回() {
    let history = [BaseMessage::human("hello"), BaseMessage::ai("hi")];
    // 没有 leaked system，直接是原始历史 + 新消息
    let result_messages = vec![
        history[0].clone(),
        history[1].clone(),
        BaseMessage::human("new question"),
    ];
    let cleaned = strip_leaked_prepends(&result_messages, history.first().map(|m| m.id()));
    assert_eq!(cleaned.len(), 3, "无leaked时应正常返回所有消息");
    assert_eq!(cleaned[0].id(), history[0].id());
}
