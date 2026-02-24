use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};

use kelvin_brain::OpenClawBrain;
use kelvin_core::{
    AgentEvent, AgentEventData, AgentRunRequest, Brain, EventSink, KelvinError, KelvinResult,
    LifecyclePhase, MemoryEmbeddingProbeResult, MemoryProviderStatus, MemoryReadParams,
    MemoryReadResult, MemorySearchManager, MemorySearchOptions, MemorySearchResult, MemorySource,
    ModelInput, ModelOutput, ModelProvider, SessionDescriptor, SessionMessage, SessionStore, Tool,
    ToolCall, ToolCallInput, ToolCallResult, ToolPhase, ToolRegistry,
};

#[derive(Default)]
struct RecordingEventSink {
    events: RwLock<Vec<AgentEvent>>,
}

impl RecordingEventSink {
    async fn all(&self) -> Vec<AgentEvent> {
        self.events.read().await.clone()
    }
}

#[async_trait]
impl EventSink for RecordingEventSink {
    async fn emit(&self, event: AgentEvent) -> KelvinResult<()> {
        self.events.write().await.push(event);
        Ok(())
    }
}

#[derive(Default)]
struct InMemorySessionStore {
    sessions: RwLock<HashMap<String, SessionDescriptor>>,
    messages: RwLock<HashMap<String, Vec<SessionMessage>>>,
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn upsert_session(&self, session: SessionDescriptor) -> KelvinResult<()> {
        self.sessions
            .write()
            .await
            .insert(session.session_id.clone(), session);
        Ok(())
    }

    async fn get_session(&self, session_id: &str) -> KelvinResult<Option<SessionDescriptor>> {
        Ok(self.sessions.read().await.get(session_id).cloned())
    }

    async fn append_message(&self, session_id: &str, message: SessionMessage) -> KelvinResult<()> {
        self.messages
            .write()
            .await
            .entry(session_id.to_string())
            .or_default()
            .push(message);
        Ok(())
    }

    async fn history(&self, session_id: &str) -> KelvinResult<Vec<SessionMessage>> {
        Ok(self
            .messages
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default())
    }
}

#[derive(Default)]
struct StaticMemory;

#[async_trait]
impl MemorySearchManager for StaticMemory {
    async fn search(
        &self,
        _query: &str,
        _opts: MemorySearchOptions,
    ) -> KelvinResult<Vec<MemorySearchResult>> {
        Ok(vec![MemorySearchResult {
            path: "MEMORY.md".to_string(),
            start_line: 1,
            end_line: 1,
            score: 1.0,
            snippet: "router vlan10".to_string(),
            source: MemorySource::Memory,
            citation: Some("MEMORY.md#1".to_string()),
        }])
    }

    async fn read_file(&self, _params: MemoryReadParams) -> KelvinResult<MemoryReadResult> {
        Ok(MemoryReadResult {
            text: String::new(),
            path: "MEMORY.md".to_string(),
        })
    }

    fn status(&self) -> MemoryProviderStatus {
        MemoryProviderStatus::default()
    }

    async fn probe_embedding_availability(&self) -> KelvinResult<MemoryEmbeddingProbeResult> {
        Ok(MemoryEmbeddingProbeResult {
            ok: false,
            error: Some("not enabled".to_string()),
        })
    }

    async fn probe_vector_availability(&self) -> KelvinResult<bool> {
        Ok(false)
    }
}

#[derive(Clone)]
struct StubModelProvider {
    delay_ms: u64,
    output: ModelOutput,
}

#[async_trait]
impl ModelProvider for StubModelProvider {
    fn provider_name(&self) -> &str {
        "stub"
    }

    fn model_name(&self) -> &str {
        "stub-model"
    }

    async fn infer(&self, _input: ModelInput) -> KelvinResult<ModelOutput> {
        if self.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        }
        Ok(self.output.clone())
    }
}

struct MapToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl MapToolRegistry {
    fn from_tools(tools: Vec<Arc<dyn Tool>>) -> Self {
        let mut map = HashMap::new();
        for tool in tools {
            map.insert(tool.name().to_string(), tool);
        }
        Self { tools: map }
    }
}

impl ToolRegistry for MapToolRegistry {
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

struct RecordingTool {
    name: String,
    visible: String,
    calls: Arc<Mutex<Vec<String>>>,
}

impl RecordingTool {
    fn new(name: &str, visible: &str, calls: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            name: name.to_string(),
            visible: visible.to_string(),
            calls,
        }
    }
}

#[async_trait]
impl Tool for RecordingTool {
    fn name(&self) -> &str {
        &self.name
    }

    async fn call(&self, _input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        self.calls.lock().await.push(self.name.clone());
        Ok(ToolCallResult {
            summary: format!("{} done", self.name),
            output: Some(self.visible.clone()),
            visible_text: Some(self.visible.clone()),
            is_error: false,
        })
    }
}

fn request(prompt: &str, timeout_ms: Option<u64>) -> AgentRunRequest {
    AgentRunRequest {
        run_id: "run-1".to_string(),
        session_id: "session-1".to_string(),
        session_key: "session-1".to_string(),
        workspace_dir: ".".to_string(),
        prompt: prompt.to_string(),
        extra_system_prompt: None,
        timeout_ms,
        memory_query: None,
    }
}

fn tool_call(id: &str, name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments,
    }
}

#[tokio::test]
async fn e2e_events_are_complete_and_ordered_and_tool_execution_is_deterministic() {
    let event_sink = Arc::new(RecordingEventSink::default());
    let session_store = Arc::new(InMemorySessionStore::default());
    let tool_calls = Arc::new(Mutex::new(Vec::new()));

    let tools = Arc::new(MapToolRegistry::from_tools(vec![
        Arc::new(RecordingTool::new(
            "first",
            "first-output",
            tool_calls.clone(),
        )),
        Arc::new(RecordingTool::new(
            "second",
            "second-output",
            tool_calls.clone(),
        )),
    ]));

    let model = Arc::new(StubModelProvider {
        delay_ms: 0,
        output: ModelOutput {
            assistant_text: "assistant-response".to_string(),
            stop_reason: Some("tool_calls".to_string()),
            tool_calls: vec![
                tool_call("1", "first", json!({"x": 1})),
                tool_call("2", "second", json!({"x": 2})),
            ],
            usage: None,
        },
    });

    let brain = OpenClawBrain::new(
        session_store.clone(),
        Arc::new(StaticMemory),
        model,
        tools,
        event_sink.clone(),
    );

    let result = brain
        .run(request("run tools", None))
        .await
        .expect("brain run");
    let payload_text = result
        .payloads
        .iter()
        .map(|item| item.text.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        payload_text,
        vec![
            "assistant-response".to_string(),
            "first-output".to_string(),
            "second-output".to_string(),
        ]
    );

    let observed_tool_order = tool_calls.lock().await.clone();
    assert_eq!(
        observed_tool_order,
        vec!["first".to_string(), "second".to_string()]
    );

    let history = session_store
        .history("session-1")
        .await
        .expect("session history");
    assert_eq!(history.len(), 4);
    assert!(matches!(history[0].role, kelvin_core::SessionRole::User));
    assert!(matches!(history[1].role, kelvin_core::SessionRole::Tool));
    assert!(matches!(history[2].role, kelvin_core::SessionRole::Tool));
    assert!(matches!(
        history[3].role,
        kelvin_core::SessionRole::Assistant
    ));

    let events = event_sink.all().await;
    assert!(events.len() >= 7, "expected full lifecycle and tool events");

    for pair in events.windows(2) {
        assert!(
            pair[0].seq < pair[1].seq,
            "event sequence must be increasing"
        );
    }

    assert!(matches!(
        events.first().map(|event| &event.data),
        Some(AgentEventData::Lifecycle {
            phase: LifecyclePhase::Start,
            ..
        })
    ));
    assert!(matches!(
        events.last().map(|event| &event.data),
        Some(AgentEventData::Lifecycle {
            phase: LifecyclePhase::End,
            ..
        })
    ));

    let tool_phases = events
        .iter()
        .filter_map(|event| match &event.data {
            AgentEventData::Tool {
                tool_name, phase, ..
            } => Some((tool_name.clone(), phase.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_phases,
        vec![
            ("first".to_string(), ToolPhase::Start),
            ("first".to_string(), ToolPhase::End),
            ("second".to_string(), ToolPhase::Start),
            ("second".to_string(), ToolPhase::End),
        ]
    );
}

#[tokio::test]
async fn e2e_timeout_produces_typed_error_and_lifecycle_error_event() {
    let event_sink = Arc::new(RecordingEventSink::default());
    let brain = OpenClawBrain::new(
        Arc::new(InMemorySessionStore::default()),
        Arc::new(StaticMemory),
        Arc::new(StubModelProvider {
            delay_ms: 120,
            output: ModelOutput {
                assistant_text: "late-response".to_string(),
                stop_reason: Some("completed".to_string()),
                tool_calls: Vec::new(),
                usage: None,
            },
        }),
        Arc::new(MapToolRegistry::from_tools(Vec::new())),
        event_sink.clone(),
    );

    let error = brain
        .run(request("slow run", Some(20)))
        .await
        .expect_err("timeout expected");
    assert!(matches!(error, KelvinError::Timeout(_)));

    let events = event_sink.all().await;
    assert!(matches!(
        events.first().map(|event| &event.data),
        Some(AgentEventData::Lifecycle {
            phase: LifecyclePhase::Start,
            ..
        })
    ));
    assert!(matches!(
        events.last().map(|event| &event.data),
        Some(AgentEventData::Lifecycle {
            phase: LifecyclePhase::Error,
            ..
        })
    ));
}

#[tokio::test]
async fn e2e_invalid_prompt_returns_typed_input_error() {
    let event_sink = Arc::new(RecordingEventSink::default());
    let brain = OpenClawBrain::new(
        Arc::new(InMemorySessionStore::default()),
        Arc::new(StaticMemory),
        Arc::new(StubModelProvider {
            delay_ms: 0,
            output: ModelOutput {
                assistant_text: String::new(),
                stop_reason: Some("completed".to_string()),
                tool_calls: Vec::new(),
                usage: None,
            },
        }),
        Arc::new(MapToolRegistry::from_tools(Vec::new())),
        event_sink.clone(),
    );

    let error = brain
        .run(request("   ", Some(100)))
        .await
        .expect_err("invalid input expected");
    assert!(matches!(error, KelvinError::InvalidInput(_)));

    let events = event_sink.all().await;
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].data,
        AgentEventData::Lifecycle {
            phase: LifecyclePhase::Error,
            ..
        }
    ));
}
