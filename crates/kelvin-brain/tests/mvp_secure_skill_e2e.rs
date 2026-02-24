use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::RwLock;

use kelvin_brain::{EchoModelProvider, KelvinBrain, WasmSkillTool};
use kelvin_core::{
    AgentEvent, AgentEventData, AgentRunRequest, CoreRuntime, EventSink, KelvinResult,
    LifecyclePhase, MemorySearchManager, SessionDescriptor, SessionMessage, SessionStore, Tool,
    ToolPhase, ToolRegistry,
};
use kelvin_memory::MarkdownMemoryManager;

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

struct SingleToolRegistry {
    tool: Arc<dyn Tool>,
}

impl SingleToolRegistry {
    fn new(tool: Arc<dyn Tool>) -> Self {
        Self { tool }
    }
}

impl ToolRegistry for SingleToolRegistry {
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if self.tool.name() == name {
            Some(self.tool.clone())
        } else {
            None
        }
    }

    fn names(&self) -> Vec<String> {
        vec![self.tool.name().to_string()]
    }
}

fn unique_workspace(prefix: &str) -> std::path::PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("kelvin-{prefix}-{millis}"));
    std::fs::create_dir_all(&dir).expect("create workspace");
    dir
}

fn write_wasm(workspace: &Path, rel_path: &str, wat_src: &str) {
    let bytes = wat::parse_str(wat_src).expect("parse wat");
    let abs_path = workspace.join(rel_path);
    if let Some(parent) = abs_path.parent() {
        std::fs::create_dir_all(parent).expect("create wasm parent");
    }
    std::fs::write(abs_path, bytes).expect("write wasm");
}

fn request(run_id: &str, workspace: &Path, prompt: &str) -> AgentRunRequest {
    AgentRunRequest {
        run_id: run_id.to_string(),
        session_id: "session-1".to_string(),
        session_key: "session-1".to_string(),
        workspace_dir: workspace.to_string_lossy().to_string(),
        prompt: prompt.to_string(),
        extra_system_prompt: None,
        timeout_ms: Some(2_000),
        memory_query: None,
    }
}

#[tokio::test]
async fn mvp_secure_skill_run_executes_and_persists_memory() {
    let workspace = unique_workspace("mvp-success");
    write_wasm(
        &workspace,
        "skills/echo.wasm",
        r#"
        (module
          (import "claw" "send_message" (func $send_message (param i32) (result i32)))
          (func (export "run") (result i32)
            i32.const 42
            call $send_message
            drop
            i32.const 0
          )
        )
        "#,
    );

    let event_sink = Arc::new(RecordingEventSink::default());
    let memory_manager: Arc<dyn MemorySearchManager> =
        Arc::new(MarkdownMemoryManager::new(&workspace));
    let brain = KelvinBrain::new(
        Arc::new(InMemorySessionStore::default()),
        memory_manager.clone(),
        Arc::new(EchoModelProvider::new("echo", "echo-model")),
        Arc::new(SingleToolRegistry::new(Arc::new(WasmSkillTool::default()))),
        event_sink.clone(),
    );

    let runtime = CoreRuntime::new(Arc::new(brain));
    let prompt = r#"[[tool:wasm_skill {"wasm_path":"skills/echo.wasm","policy_preset":"locked_down","memory_append_path":"memory/mvp.md","memory_entry":"secure-run-ok"}]]"#;
    runtime
        .submit(request("run-mvp-success", &workspace, prompt))
        .await
        .expect("submit");

    let outcome = runtime
        .wait_for_outcome("run-mvp-success", 2_000)
        .await
        .expect("wait outcome");
    let result = match outcome {
        kelvin_core::RunOutcome::Completed(result) => result,
        other => panic!("expected completed outcome, got {other:?}"),
    };
    assert!(result
        .payloads
        .iter()
        .any(|payload| payload.text.contains("wasm skill exit=0 calls=1")));

    let memory_file = workspace.join("memory/mvp.md");
    let memory_text = std::fs::read_to_string(&memory_file).expect("memory file");
    assert!(memory_text.contains("secure-run-ok"));

    let search_hits = memory_manager
        .search("secure-run-ok", Default::default())
        .await
        .expect("memory search");
    assert!(!search_hits.is_empty(), "expected persisted memory hit");
    assert_eq!(search_hits[0].path, "memory/mvp.md");

    let events = event_sink.all().await;
    assert!(events.len() >= 4, "expected lifecycle and tool events");
    for pair in events.windows(2) {
        assert!(pair[0].seq < pair[1].seq, "event seq must increase");
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
            } if tool_name == "wasm_skill" => Some(phase.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_phases, vec![ToolPhase::Start, ToolPhase::End]);
}

#[tokio::test]
async fn mvp_secure_skill_run_denies_disallowed_capability() {
    let workspace = unique_workspace("mvp-denied");
    write_wasm(
        &workspace,
        "skills/fs.wasm",
        r#"
        (module
          (import "claw" "fs_read" (func $fs_read (param i32) (result i32)))
          (func (export "run") (result i32)
            i32.const 1
            call $fs_read
          )
        )
        "#,
    );

    let event_sink = Arc::new(RecordingEventSink::default());
    let brain = KelvinBrain::new(
        Arc::new(InMemorySessionStore::default()),
        Arc::new(MarkdownMemoryManager::new(&workspace)),
        Arc::new(EchoModelProvider::new("echo", "echo-model")),
        Arc::new(SingleToolRegistry::new(Arc::new(WasmSkillTool::default()))),
        event_sink.clone(),
    );
    let runtime = CoreRuntime::new(Arc::new(brain));

    let prompt = r#"[[tool:wasm_skill {"wasm_path":"skills/fs.wasm","policy_preset":"locked_down","memory_append_path":"memory/mvp.md","memory_entry":"should-not-write"}]]"#;
    runtime
        .submit(request("run-mvp-denied", &workspace, prompt))
        .await
        .expect("submit");

    let outcome = runtime
        .wait_for_outcome("run-mvp-denied", 2_000)
        .await
        .expect("wait outcome");
    let error = match outcome {
        kelvin_core::RunOutcome::Failed(error) => error,
        other => panic!("expected failed outcome, got {other:?}"),
    };
    assert!(error.contains("denied by sandbox policy"));

    let memory_file = workspace.join("memory/mvp.md");
    assert!(
        !memory_file.exists(),
        "memory append should not happen on denied capability"
    );

    let events = event_sink.all().await;
    assert!(matches!(
        events.last().map(|event| &event.data),
        Some(AgentEventData::Lifecycle {
            phase: LifecyclePhase::Error,
            ..
        })
    ));
}
