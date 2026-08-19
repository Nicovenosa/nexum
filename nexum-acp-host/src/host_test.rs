use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use nexum_acp::{
    provider::{LlmProvider, NexumConfig, ProviderConfig, ProviderModels},
    runtime::{
        CapabilityState, RuntimeCapabilities, RuntimeHealth, RuntimeHealthState, RuntimeIdentity,
        RuntimeIdentityInput, RuntimeMetadata,
    },
    server::{AcpServerConfig, NoopPredictionPolicy},
    session::SessionManager,
    transport::{local::LocalAcpTransport, types::IncomingMessage, AcpTransport},
};
use nexum_agent::{
    interaction::ChannelState,
    thread::{FilesystemThreadStore, ThreadStore},
};
use nexum_middlewares::{
    cron::CronControlClient,
    hitl::shared_mode::{PermissionMode, SharedPermissionMode},
    tool_search::ToolSearchIndex,
};
use nexum_tui::{
    acp_client::{AcpClientTransport, AcpTuiClient},
    voice::acp_turn::{
        VoiceAcpClient, VoiceHudBridge, VoiceRouteDecision, VoiceSessionStore, VoiceTurnController,
        VoiceTurnError,
    },
};
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    process::Command,
    task::JoinHandle,
    time::{sleep, timeout},
};

use crate::{config::HostConfig, host};

const REAL_OLLAMA_PROMPT: &str = "Respondé únicamente con: NEXUM_ACP_E2E_OK";

#[derive(Debug)]
struct DirectOllamaTrace {
    elapsed: Duration,
    first_sse: Option<Duration>,
    first_text: Option<Duration>,
    terminal: Option<Duration>,
    text_chunks: u32,
    reasoning_chunks: u32,
    finish_reason_events: u32,
    done_markers: u32,
    raw_sample: Option<String>,
}

#[derive(Debug)]
struct PromptSectionMetrics {
    system_total_chars: usize,
    base_prompt_chars: usize,
    additional_system_chars: usize,
    system_messages: usize,
    identity: bool,
    locale: bool,
    objective: bool,
    output: bool,
    subagent: bool,
    skills: bool,
    project: bool,
    docs: bool,
    tools: bool,
}

struct MockProvider {
    base_url: String,
    requests: Arc<AtomicUsize>,
    request_tool_counts: Arc<parking_lot::Mutex<Vec<usize>>>,
    task: JoinHandle<()>,
}

#[derive(Debug)]
enum ProviderProxyEvent {
    Request {
        elapsed: Duration,
        bytes: usize,
        messages: usize,
        prompt_metrics: PromptSectionMetrics,
        tools: usize,
        max_tokens: Option<u64>,
        streaming: Option<bool>,
    },
    FirstUpstreamByte {
        elapsed: Duration,
    },
}

struct OllamaProxy {
    base_url: String,
    events: tokio::sync::mpsc::UnboundedReceiver<ProviderProxyEvent>,
    task: JoinHandle<()>,
}

impl OllamaProxy {
    async fn start(upstream: &str, started: Instant) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let upstream = upstream
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();
        let (event_tx, events) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let Ok((mut client, _)) = listener.accept().await else {
                return;
            };
            let mut request = Vec::new();
            let mut header_end = None;
            let mut content_length = None;
            loop {
                let mut chunk = [0_u8; 8192];
                let Ok(read) = client.read(&mut chunk).await else {
                    return;
                };
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&chunk[..read]);
                if header_end.is_none() {
                    header_end = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|i| i + 4);
                    if let Some(end) = header_end {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        content_length = headers.lines().find_map(|line| {
                            line.split_once(':').and_then(|(name, value)| {
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                        });
                    }
                }
                if let (Some(end), Some(length)) = (header_end, content_length) {
                    if request.len() >= end.saturating_add(length) {
                        break;
                    }
                }
            }

            let body = header_end
                .and_then(|end| std::str::from_utf8(&request[end..]).ok())
                .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
                .unwrap_or_default();
            let system_messages = body["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|message| message["role"] == "system")
                .filter_map(|message| message["content"].as_str())
                .collect::<Vec<_>>();
            let _ = event_tx.send(ProviderProxyEvent::Request {
                elapsed: started.elapsed(),
                bytes: request.len(),
                messages: body["messages"].as_array().map_or(0, Vec::len),
                prompt_metrics: prompt_section_metrics(&system_messages),
                tools: body["tools"].as_array().map_or(0, Vec::len),
                max_tokens: body["max_tokens"].as_u64(),
                streaming: body["stream"].as_bool(),
            });

            let Ok(mut upstream_stream) = TcpStream::connect(&upstream).await else {
                return;
            };
            if upstream_stream.write_all(&request).await.is_err() {
                return;
            }
            let mut first_byte_sent = false;
            loop {
                let mut chunk = [0_u8; 8192];
                let Ok(read) = upstream_stream.read(&mut chunk).await else {
                    return;
                };
                if read == 0 {
                    return;
                }
                if !first_byte_sent {
                    first_byte_sent = true;
                    let _ = event_tx.send(ProviderProxyEvent::FirstUpstreamByte {
                        elapsed: started.elapsed(),
                    });
                }
                if client.write_all(&chunk[..read]).await.is_err() {
                    return;
                }
            }
        });
        Self {
            base_url,
            events,
            task,
        }
    }
}

fn prompt_section_metrics(system_messages: &[&str]) -> PromptSectionMetrics {
    let system_total_chars = system_messages
        .iter()
        .map(|message| message.chars().count())
        .sum();
    let system_prompt = system_messages
        .iter()
        .copied()
        .find(|message| message.contains("You are Nexum Agent"))
        .unwrap_or_default();
    let base_prompt = system_prompt
        .split("\n\n## Git Attribution")
        .next()
        .unwrap_or_default();
    let normalized = base_prompt.to_ascii_lowercase();
    PromptSectionMetrics {
        system_total_chars,
        base_prompt_chars: base_prompt.chars().count(),
        additional_system_chars: system_total_chars.saturating_sub(base_prompt.chars().count()),
        system_messages: system_messages.len(),
        identity: base_prompt.contains("You are Nexum Agent"),
        locale: base_prompt.contains("Always respond in "),
        objective: base_prompt.contains("user's stated objective"),
        output: base_prompt.contains("requested output"),
        subagent: normalized.contains("subagent"),
        skills: normalized.contains("skills"),
        project: normalized.contains("project"),
        docs: normalized.contains("docs"),
        tools: normalized.contains("tool"),
    }
}

impl Drop for OllamaProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockProvider {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let request_tool_counts = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let counter = Arc::clone(&requests);
        let tool_counts = Arc::clone(&request_tool_counts);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let counter = Arc::clone(&counter);
                let tool_counts = Arc::clone(&tool_counts);
                tokio::spawn(async move {
                    let mut request = vec![0; 65_536];
                    let size = socket.read(&mut request).await.unwrap_or_default();
                    let request = String::from_utf8_lossy(&request[..size]);
                    counter.fetch_add(1, Ordering::SeqCst);
                    let tool_count = request
                        .split_once("\r\n\r\n")
                        .and_then(|(_, body)| serde_json::from_str::<serde_json::Value>(body).ok())
                        .and_then(|body| body["tools"].as_array().map(Vec::len))
                        .unwrap_or_default();
                    tool_counts.lock().push(tool_count);
                    let slow = request.contains("cancel-me");
                    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
                    socket.write_all(headers.as_bytes()).await.unwrap();
                    socket.flush().await.unwrap();
                    if slow {
                        socket
                            .write_all(
                                b"data: {\"id\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\"waiting\"},\"finish_reason\":null}]}\n\n",
                            )
                            .await
                            .unwrap();
                        socket.flush().await.unwrap();
                        sleep(Duration::from_secs(10)).await;
                    }
                    socket
                        .write_all(
                            b"data: {\"id\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"respuesta mock\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                        )
                        .await
                        .unwrap();
                });
            }
        });
        Self {
            base_url,
            requests,
            request_tool_counts,
            task,
        }
    }
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn host_config(tmp: &tempfile::TempDir, nexum_config: NexumConfig) -> HostConfig {
    let provider = LlmProvider::from_config(&nexum_config).unwrap();
    let thread_store: Arc<dyn ThreadStore> =
        Arc::new(FilesystemThreadStore::new(tmp.path().join("threads")));
    let permission_mode = SharedPermissionMode::new(PermissionMode::Bypass);
    let config = Arc::new(parking_lot::RwLock::new(nexum_config));
    let provider = Arc::new(parking_lot::RwLock::new(provider));
    let session_manager = SessionManager::new(
        Arc::clone(&thread_store),
        provider.read().clone(),
        Arc::new(config.read().clone()),
        Arc::clone(&permission_mode),
        None,
    );
    HostConfig {
        server: AcpServerConfig {
            provider,
            nexum_config: config,
            permission_mode,
            cron_control: CronControlClient::unavailable(),
            mcp_pool: None,
            channel_state: Some(ChannelState::new()),
            plugin_skill_roots: Vec::new(),
            plugin_agent_dirs: Vec::new(),
            plugin_hooks: Vec::new(),
            hook_groups: Vec::new(),
            plugin_lsp_servers: Vec::new(),
            tool_search_index: Arc::new(ToolSearchIndex::new()),
            shared_tools: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            thread_store,
            langfuse_session: None,
            config_path: tmp.path().join("config.json"),
            session_manager,
            prediction_policy: Arc::new(NoopPredictionPolicy),
            pending_interaction_broker: None,
            runtime: RuntimeMetadata {
                identity: Arc::new(parking_lot::RwLock::new(RuntimeIdentity::new(
                    RuntimeIdentityInput::default(),
                ))),
                capabilities: RuntimeCapabilities::new(
                    "nexum.acp.capabilities/v1",
                    [(
                        "cron",
                        CapabilityState::Unavailable {
                            reason: "disabled in E2E".into(),
                        },
                    )],
                    std::iter::empty::<(&str, Vec<String>)>(),
                ),
                health: RuntimeHealth::new(RuntimeHealthState::Ready),
            },
        },
        cron: None,
    }
}

fn mock_host_config(tmp: &tempfile::TempDir, base_url: &str) -> HostConfig {
    let provider_config = ProviderConfig {
        id: "mock".into(),
        provider_type: "openai".into(),
        api_key: "local".into(),
        base_url: base_url.into(),
        models: ProviderModels {
            sonnet: "mock-model".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut nexum_config = NexumConfig::default();
    nexum_config.config.active_provider_id = "mock".into();
    nexum_config.config.active_alias = "sonnet".into();
    nexum_config.config.providers = vec![provider_config];
    host_config(tmp, nexum_config)
}

fn configured_ollama_config() -> anyhow::Result<NexumConfig> {
    let mut config = nexum_acp::provider::load()?;
    let Some((provider_id, alias)) = config
        .config
        .providers
        .iter()
        .filter(|provider| provider.base_url.contains("127.0.0.1:11434"))
        .find_map(|provider| {
            [
                ("haiku", &provider.models.haiku),
                ("sonnet", &provider.models.sonnet),
                ("opus", &provider.models.opus),
            ]
            .into_iter()
            .find(|(_, model)| !model.trim().is_empty())
            .map(|(alias, _)| (provider.id.clone(), alias.to_string()))
        })
    else {
        anyhow::bail!("no configured local Ollama provider with a model alias")
    };
    config.config.active_provider_id = provider_id;
    config.config.active_alias = alias;
    config.config.thinking = None;
    if LlmProvider::from_config(&config).is_none() {
        anyhow::bail!("configured local Ollama provider is not usable")
    }
    Ok(config)
}

fn configured_ollama_connection() -> anyhow::Result<(NexumConfig, String, String)> {
    let config = configured_ollama_config()?;
    let provider = LlmProvider::from_config(&config)
        .ok_or_else(|| anyhow::anyhow!("configured local Ollama provider is not usable"))?;
    match provider {
        LlmProvider::OpenAi {
            base_url, model, ..
        } => Ok((config, base_url, model)),
        LlmProvider::Anthropic { .. } => {
            anyhow::bail!("configured local Ollama provider is not OpenAI-compatible")
        }
    }
}

async fn ollama_ps(model: &str) -> String {
    let output = Command::new("ollama").arg("ps").output().await.ok();
    output
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|output| {
            output
                .lines()
                .filter(|line| line.contains(model))
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .filter(|output| !output.is_empty())
        .unwrap_or_else(|| "not-listed".into())
}

async fn stop_ollama_model(model: &str) -> anyhow::Result<()> {
    let status = Command::new("ollama")
        .arg("stop")
        .arg(model)
        .status()
        .await?;
    anyhow::ensure!(status.success(), "ollama stop returned {status}");
    Ok(())
}

async fn wait_for_ollama_unload(model: &str) -> bool {
    for _ in 0..50 {
        if ollama_ps(model).await == "not-listed" {
            return true;
        }
        sleep(Duration::from_millis(200)).await;
    }
    false
}

fn sanitized_sse_sample(data: &str) -> String {
    if data == "[DONE]" {
        return "[DONE]".into();
    }
    let parsed: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
    let choice = parsed
        .get("choices")
        .and_then(|choices| choices.get(0))
        .cloned()
        .unwrap_or_default();
    let delta = choice.get("delta").cloned().unwrap_or_default();
    let text = delta
        .get("content")
        .or_else(|| delta.get("reasoning_content"))
        .or_else(|| delta.get("reasoning"))
        .and_then(serde_json::Value::as_str)
        .map(|text| text.chars().take(80).collect::<String>());
    serde_json::json!({
        "delta_keys": delta.as_object().map(|value| value.keys().cloned().collect::<Vec<_>>()).unwrap_or_default(),
        "text": text,
        "finish_reason": choice.get("finish_reason"),
        "has_usage": parsed.get("usage").is_some(),
    })
    .to_string()
}

async fn direct_ollama_stream(
    base_url: &str,
    model: &str,
    prompt: &str,
) -> anyhow::Result<DirectOllamaTrace> {
    let request = json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "stream": true,
        "stream_options": {"include_usage": true},
        "max_tokens": 32000,
    });
    let endpoint = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let started = Instant::now();
    let mut child = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--no-buffer",
            "--max-time",
            "120",
            "-H",
            "Content-Type: application/json",
            "--data",
        ])
        .arg(request.to_string())
        .arg(endpoint)
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("curl stdout is piped");
    let mut lines = BufReader::new(stdout).lines();
    let mut trace = DirectOllamaTrace {
        elapsed: Duration::ZERO,
        first_sse: None,
        first_text: None,
        terminal: None,
        text_chunks: 0,
        reasoning_chunks: 0,
        finish_reason_events: 0,
        done_markers: 0,
        raw_sample: None,
    };
    while let Some(line) = lines.next_line().await? {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let elapsed = started.elapsed();
        trace.first_sse.get_or_insert(elapsed);
        trace
            .raw_sample
            .get_or_insert_with(|| sanitized_sse_sample(data));
        if data == "[DONE]" {
            trace.done_markers += 1;
            trace.terminal.get_or_insert(elapsed);
            continue;
        }
        let parsed: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
        let choice = parsed
            .get("choices")
            .and_then(|choices| choices.get(0))
            .cloned()
            .unwrap_or_default();
        let delta = choice.get("delta").cloned().unwrap_or_default();
        if delta
            .get("content")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| !text.is_empty())
        {
            trace.text_chunks += 1;
            trace.first_text.get_or_insert(elapsed);
        }
        if delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| !text.is_empty())
        {
            trace.reasoning_chunks += 1;
        }
        if choice
            .get("finish_reason")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|reason| !reason.is_empty())
        {
            trace.finish_reason_events += 1;
            trace.terminal.get_or_insert(elapsed);
        }
    }
    let status = child.wait().await?;
    anyhow::ensure!(status.success(), "direct Ollama curl returned {status}");
    trace.elapsed = started.elapsed();
    Ok(trace)
}

async fn connect(socket: &std::path::Path) -> LocalAcpTransport {
    for _ in 0..200 {
        if let Ok(transport) =
            LocalAcpTransport::connect_ready(socket, Duration::from_millis(100)).await
        {
            return transport;
        }
        sleep(Duration::from_millis(20)).await;
    }
    panic!("local ACP host did not become ready")
}

fn envelope(input: &str) -> nexum_acp::task::TaskEnvelopeV1 {
    nexum_acp::task::TaskEnvelopeV1 {
        version: nexum_acp::task::TaskEnvelopeVersion::V1,
        envelope_id: "host-e2e".into(),
        source: nexum_acp::task::TaskSource::Voice,
        objective: input.into(),
        user_input: input.into(),
        session_id: String::new(),
        thread_id: String::new(),
        workspace: Some(".".into()),
        constraints: Vec::new(),
        allowed_tools: Vec::new(),
        evidence_refs: Vec::new(),
        success_criteria: Vec::new(),
        output_format: nexum_acp::task::OutputFormat::Text,
        execution_budget: Default::default(),
        evidence_policy: nexum_acp::task::EvidencePolicy {
            require_evidence: false,
            minimum_evidence_refs: 0,
            allow_unverified_output: true,
        },
        priority: nexum_acp::task::TaskPriority::Normal,
        risk: nexum_acp::task::TaskRisk::Low,
        sanitized_metadata: Default::default(),
    }
}

#[tokio::test]
async fn test_local_host_e2e_voice_tui_reconnect_cancel_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("acp.sock");
    let mock = MockProvider::start().await;
    let host_task = tokio::spawn(host::run(
        socket.clone(),
        mock_host_config(&temp, &mock.base_url),
    ));

    let voice_transport: Arc<dyn AcpClientTransport> = Arc::new(connect(&socket).await);
    let store = VoiceSessionStore::in_memory();
    let mut voice = VoiceTurnController::new(
        VoiceAcpClient::from_transport(voice_transport),
        store.clone(),
        VoiceHudBridge::default(),
    );
    let answer = voice
        .execute(
            VoiceRouteDecision::Escalate {
                envelope: envelope("first-answer"),
                reason: "E2E".into(),
            },
            ".",
        )
        .await
        .unwrap();
    assert_eq!(answer.speakable, "respuesta mock");
    assert_eq!(
        mock.request_tool_counts.lock().first().copied(),
        Some(0),
        "un envelope de voz sin allowed_tools no debe exponer schemas al modelo"
    );
    let session = store.get().unwrap();

    let thread_store = FilesystemThreadStore::new(temp.path().join("threads"));
    let persisted = thread_store
        .load_messages(&session.thread_id)
        .await
        .unwrap();
    assert!(persisted
        .iter()
        .any(|message| message.content().contains("respuesta mock")));

    let tui_transport: Arc<dyn AcpClientTransport> = Arc::new(connect(&socket).await);
    let (tui, _) = AcpTuiClient::new(tui_transport);
    tui.load_session(&session.session_id, ".", None)
        .await
        .unwrap();
    assert!(tui
        .request("session/list", json!({"cwd":"."}))
        .await
        .unwrap()["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["sessionId"] == session.session_id));
    tui.close().await.unwrap();

    let reconnected: Arc<dyn AcpClientTransport> = Arc::new(connect(&socket).await);
    let (tui_reconnected, _) = AcpTuiClient::new(reconnected);
    tui_reconnected
        .load_session(&session.session_id, ".", None)
        .await
        .unwrap();

    let cancel_store = VoiceSessionStore::in_memory();
    cancel_store.replace(session.clone()).unwrap();
    let cancel_transport: Arc<dyn AcpClientTransport> = Arc::new(connect(&socket).await);
    let cancel_task = tokio::spawn(async move {
        let mut controller = VoiceTurnController::new(
            VoiceAcpClient::from_transport(cancel_transport),
            cancel_store,
            VoiceHudBridge::default(),
        );
        controller
            .execute(
                VoiceRouteDecision::Escalate {
                    envelope: envelope("cancel-me"),
                    reason: "E2E cancel".into(),
                },
                ".",
            )
            .await
    });
    for _ in 0..50 {
        if mock.requests.load(Ordering::SeqCst) >= 2 {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert!(mock.requests.load(Ordering::SeqCst) >= 2);
    let canceller = connect(&socket).await;
    canceller
        .send_notification("session/cancel", json!({"sessionId": session.session_id}))
        .await
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), cancel_task)
            .await
            .unwrap()
            .unwrap(),
        Err(VoiceTurnError::Cancelled)
    ));

    host_task.abort();
    let _ = host_task.await;
    let restarted = tokio::spawn(host::run(
        socket.clone(),
        mock_host_config(&temp, &mock.base_url),
    ));
    let after_restart: Arc<dyn AcpClientTransport> = Arc::new(connect(&socket).await);
    let (tui_after_restart, _) = AcpTuiClient::new(after_restart);
    tui_after_restart
        .load_session(&session.session_id, ".", None)
        .await
        .unwrap();
    assert!(tui_after_restart
        .request("session/list", json!({"cwd":"."}))
        .await
        .unwrap()["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["sessionId"] == session.session_id));
    restarted.abort();
}

#[tokio::test]
#[ignore = "requires a running local Ollama and configured Ollama provider"]
async fn test_real_ollama_voice_host_e2e_returns_exact_sentinel_once() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("acp.sock");
    let t0 = Instant::now();
    let (mut ollama_config, base_url, _) = configured_ollama_connection().unwrap();
    let mut proxy = OllamaProxy::start(&base_url, t0).await;
    let active_provider = ollama_config.config.active_provider_id.clone();
    ollama_config
        .config
        .providers
        .iter_mut()
        .find(|provider| provider.id == active_provider)
        .expect("active Ollama provider must remain configured")
        .base_url = proxy.base_url.clone();
    let host_task = tokio::spawn(host::run(socket.clone(), host_config(&temp, ollama_config)));

    let transport: Arc<dyn AcpClientTransport> = Arc::new(connect(&socket).await);
    let store = VoiceSessionStore::in_memory();
    let mut voice = VoiceTurnController::new(
        VoiceAcpClient::from_transport(transport),
        store.clone(),
        VoiceHudBridge::default(),
    );
    let answer = voice
        .execute(
            VoiceRouteDecision::Escalate {
                envelope: envelope(REAL_OLLAMA_PROMPT),
                reason: "real Ollama E2E".into(),
            },
            temp.path().to_str().unwrap(),
        )
        .await;

    let mut prompt_metrics = None;
    let mut provider_requests = 0;
    let mut upstream_first_bytes = 0;
    let mut model_tool_schemas = 0;
    while let Ok(event) = proxy.events.try_recv() {
        match event {
            ProviderProxyEvent::Request {
                prompt_metrics: metrics,
                tools,
                ..
            } => {
                provider_requests += 1;
                prompt_metrics = Some(metrics);
                model_tool_schemas += tools;
            }
            ProviderProxyEvent::FirstUpstreamByte { .. } => upstream_first_bytes += 1,
        }
    }
    if let Some(prompt_metrics) = prompt_metrics.as_ref() {
        println!(
            "local_micro_prompt system_total_chars={} base_prompt_chars={} additional_system_chars={} system_messages={} identity={} locale={} objective={} output={} subagent={} skills={} project={} docs={} tools={}",
            prompt_metrics.system_total_chars,
            prompt_metrics.base_prompt_chars,
            prompt_metrics.additional_system_chars,
            prompt_metrics.system_messages,
            prompt_metrics.identity,
            prompt_metrics.locale,
            prompt_metrics.objective,
            prompt_metrics.output,
            prompt_metrics.subagent,
            prompt_metrics.skills,
            prompt_metrics.project,
            prompt_metrics.docs,
            prompt_metrics.tools,
        );
    }
    println!(
        "local_micro_e2e provider_requests={} upstream_first_bytes={} model_tool_schemas={} outcome={}",
        provider_requests,
        upstream_first_bytes,
        model_tool_schemas,
        if answer.is_ok() { "ok" } else { "timed_out" },
    );
    let answer = answer.unwrap();

    assert_eq!(answer.speakable.trim(), "NEXUM_ACP_E2E_OK");
    assert_eq!(answer.speakable.matches("NEXUM_ACP_E2E_OK").count(), 1);
    assert_eq!(
        model_tool_schemas, 0,
        "allowed_tools=[] no debe exponer schemas"
    );
    let prompt_metrics =
        prompt_metrics.expect("real E2E reached a response without prompt metrics");
    assert!(prompt_metrics.identity && prompt_metrics.objective && prompt_metrics.output);
    assert!(
        !prompt_metrics.subagent
            && !prompt_metrics.skills
            && !prompt_metrics.project
            && !prompt_metrics.docs
            && !prompt_metrics.tools
    );
    let session = store.get().unwrap();
    let thread_store = FilesystemThreadStore::new(temp.path().join("threads"));
    let persisted = thread_store
        .load_messages(&session.thread_id)
        .await
        .unwrap();
    assert_eq!(
        persisted
            .iter()
            .filter(|message| matches!(message, nexum_agent::messages::BaseMessage::Ai { .. }))
            .filter(|message| message.content().contains("NEXUM_ACP_E2E_OK"))
            .count(),
        1
    );

    host_task.abort();
    let _ = host_task.await;
}

#[tokio::test]
#[ignore = "requires a running local Ollama and configured Ollama provider"]
async fn test_real_ollama_direct_cold_warm_latency() {
    timeout(Duration::from_secs(120), async {
        let (_, base_url, model) = configured_ollama_connection().unwrap();
        println!(
            "direct config endpoint={} model={} prompt_chars={} max_tokens=32000 stream=true",
            base_url,
            model,
            REAL_OLLAMA_PROMPT.chars().count(),
        );

        let before_stop = ollama_ps(&model).await;
        stop_ollama_model(&model).await.unwrap();
        assert!(wait_for_ollama_unload(&model).await, "Ollama did not unload the model within 10 seconds");
        println!("direct cold unload before={} after=not-listed", before_stop);

        let cold = direct_ollama_stream(&base_url, &model, REAL_OLLAMA_PROMPT)
            .await
            .unwrap();
        println!(
            "direct_cold total_ms={} first_sse_ms={:?} ttft_ms={:?} finish={} done={} text_chunks={} reasoning_chunks={} raw={}",
            cold.elapsed.as_millis(),
            cold.first_sse.map(|value| value.as_millis()),
            cold.first_text.map(|value| value.as_millis()),
            cold.finish_reason_events,
            cold.done_markers,
            cold.text_chunks,
            cold.reasoning_chunks,
            cold.raw_sample.unwrap_or_else(|| "none".into()),
        );
        assert_eq!(cold.finish_reason_events, 1, "cold direct stream must have one finish reason");
        assert_eq!(cold.done_markers, 1, "cold direct stream must have one [DONE] marker");

        let warm = direct_ollama_stream(&base_url, &model, REAL_OLLAMA_PROMPT)
            .await
            .unwrap();
        println!(
            "direct_warm total_ms={} first_sse_ms={:?} ttft_ms={:?} finish={} done={} text_chunks={} reasoning_chunks={} raw={}",
            warm.elapsed.as_millis(),
            warm.first_sse.map(|value| value.as_millis()),
            warm.first_text.map(|value| value.as_millis()),
            warm.finish_reason_events,
            warm.done_markers,
            warm.text_chunks,
            warm.reasoning_chunks,
            warm.raw_sample.unwrap_or_else(|| "none".into()),
        );
        assert_eq!(warm.finish_reason_events, 1, "warm direct stream must have one finish reason");
        assert_eq!(warm.done_markers, 1, "warm direct stream must have one [DONE] marker");
    })
    .await
    .expect("direct cold/warm diagnostic must stay within 120 seconds");
}

#[tokio::test]
#[ignore = "requires a running local Ollama and configured Ollama provider"]
async fn test_real_ollama_voice_latency_diagnostic_120s() {
    timeout(Duration::from_secs(120), async {
        let t0 = Instant::now();
        let (mut ollama_config, base_url, model) = configured_ollama_connection().unwrap();
        println!(
            "T0 config endpoint={} model={} prompt_chars={} max_tokens=32000 stream=true",
            base_url,
            model,
            REAL_OLLAMA_PROMPT.chars().count(),
        );
        let mut proxy = OllamaProxy::start(&base_url, t0).await;
        let active_provider = ollama_config.config.active_provider_id.clone();
        ollama_config
            .config
            .providers
            .iter_mut()
            .find(|provider| provider.id == active_provider)
            .expect("active Ollama provider must remain configured")
            .base_url = proxy.base_url.clone();

        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("acp.sock");
        let host_task = tokio::spawn(host::run(socket.clone(), host_config(&temp, ollama_config)));
        println!("T1 host_spawned_ms={}", t0.elapsed().as_millis());
        let transport = Arc::new(connect(&socket).await);
        println!("T2 connected_ms={}", t0.elapsed().as_millis());

        let identity = transport
            .send_request("runtime/identity", json!({}))
            .await
            .unwrap();
        println!(
            "T3 runtime_identity_ms={} provider={:?} model={:?}",
            t0.elapsed().as_millis(),
            identity.get("provider").and_then(serde_json::Value::as_str),
            identity.get("model").and_then(serde_json::Value::as_str),
        );
        let session = transport
            .send_request("session/new", json!({"cwd": temp.path()}))
            .await
            .unwrap();
        let session_id = session["sessionId"].as_str().unwrap().to_string();
        let thread_id = session["threadId"].as_str().unwrap().to_string();
        println!("T4 session_ready_ms={}", t0.elapsed().as_millis());

        let mut diagnostic_envelope = envelope(REAL_OLLAMA_PROMPT);
        diagnostic_envelope.execution_budget.wall_time_ms = Some(120_000);
        let prompt_transport = Arc::clone(&transport);
        let prompt_session_id = session_id.clone();
        let prompt_task = tokio::spawn(async move {
            prompt_transport
                .send_request(
                    "session/prompt",
                    json!({
                        "sessionId": prompt_session_id,
                        "message": {"role": "user", "content": REAL_OLLAMA_PROMPT},
                        "taskEnvelope": diagnostic_envelope,
                    }),
                )
                .await
        });
        tokio::pin!(prompt_task);
        println!("T5 prompt_dispatched_ms={}", t0.elapsed().as_millis());

        let mut first_notification = None;
        let mut first_text = None;
        let mut done_at = None;
        let mut text_chunks = 0_u32;
        let mut done_events = 0_u32;
        let mut received_text = String::new();
        let prompt_response = loop {
            tokio::select! {
                response = &mut prompt_task => break response.unwrap().unwrap(),
                provider_event = proxy.events.recv() => {
                    match provider_event {
                        Some(ProviderProxyEvent::Request { elapsed, bytes, messages, prompt_metrics, tools, max_tokens, streaming }) => {
                            println!(
                                "T6 provider_request_ms={} bytes={} messages={} system_total_chars={} base_prompt_chars={} additional_system_chars={} tools={} max_tokens={:?} stream={:?}",
                                elapsed.as_millis(), bytes, messages, prompt_metrics.system_total_chars, prompt_metrics.base_prompt_chars, prompt_metrics.additional_system_chars, tools, max_tokens, streaming,
                            );
                        }
                        Some(ProviderProxyEvent::FirstUpstreamByte { elapsed }) => {
                            println!("T7 provider_first_byte_ms={}", elapsed.as_millis());
                        }
                        None => {}
                    }
                }
                message = transport.recv() => {
                    let Some(IncomingMessage::Notification { method, params }) = message else {
                        continue;
                    };
                    first_notification.get_or_insert_with(|| t0.elapsed());
                    match method.as_str() {
                        "session/update" => {
                            if let Some(chunk) = params
                                .get("update")
                                .and_then(|update| update.get("content"))
                                .and_then(|content| content.get("text"))
                                .and_then(serde_json::Value::as_str)
                            {
                                first_text.get_or_insert_with(|| t0.elapsed());
                                text_chunks += 1;
                                received_text.push_str(chunk);
                            }
                        }
                        "peri/agent_event_done" => {
                            done_events += 1;
                            done_at.get_or_insert_with(|| t0.elapsed());
                        }
                        _ => {}
                    }
                }
            }
        };
        assert!(prompt_response.is_object());
        println!(
            "T6/T7/T8 events first_notification_ms={:?} first_text_ms={:?} text_chunks={} terminal_done_events={} terminal_ms={:?}",
            first_notification.map(|value| value.as_millis()),
            first_text.map(|value| value.as_millis()),
            text_chunks,
            done_events,
            done_at.map(|value| value.as_millis()),
        );
        assert_eq!(done_events, 1, "the diagnostic prompt must emit one terminal done event");
        assert_eq!(received_text.matches("NEXUM_ACP_E2E_OK").count(), 1);

        println!("T9 prompt_response_ms={}", t0.elapsed().as_millis());
        let thread_store = FilesystemThreadStore::new(temp.path().join("threads"));
        let persisted = thread_store.load_messages(&thread_id).await.unwrap();
        let persisted_sentinels = persisted
            .iter()
            .filter(|message| matches!(message, nexum_agent::messages::BaseMessage::Ai { .. }))
            .filter(|message| message.content().contains("NEXUM_ACP_E2E_OK"))
            .count();
        println!(
            "T9 threadstore_ms={} messages={} sentinel_messages={}",
            t0.elapsed().as_millis(),
            persisted.len(),
            persisted_sentinels,
        );
        assert_eq!(persisted_sentinels, 1);

        transport.close().await.unwrap();
        let reconnected = connect(&socket).await;
        reconnected
            .send_request(
                "session/load",
                json!({"sessionId": session_id, "threadId": thread_id, "cwd": temp.path()}),
            )
            .await
            .unwrap();
        reconnected.close().await.unwrap();
        println!("T10 reconnect_and_load_ms={}", t0.elapsed().as_millis());
        host_task.abort();
        let _ = host_task.await;
    })
    .await
    .expect("latency diagnostic must stay within its 120 second test-only budget");
}
