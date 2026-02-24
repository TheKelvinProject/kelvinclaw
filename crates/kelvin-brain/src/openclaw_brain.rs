use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::time;

use kelvin_core::{
    now_ms, AgentEvent, AgentPayload, AgentRunMeta, AgentRunRequest, AgentRunResult, Brain,
    EventSink, KelvinError, KelvinResult, LifecyclePhase, MemorySearchManager,
    MemorySearchOptions, ModelInput, ModelProvider, SessionDescriptor, SessionMessage,
    SessionStore, ToolCallInput, ToolPhase, ToolRegistry,
};

#[derive(Clone)]
pub struct OpenClawBrain {
    session_store: Arc<dyn SessionStore>,
    memory: Arc<dyn MemorySearchManager>,
    model: Arc<dyn ModelProvider>,
    tools: Arc<dyn ToolRegistry>,
    events: Arc<dyn EventSink>,
    seq: Arc<AtomicU64>,
}

impl OpenClawBrain {
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        memory: Arc<dyn MemorySearchManager>,
        model: Arc<dyn ModelProvider>,
        tools: Arc<dyn ToolRegistry>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            session_store,
            memory,
            model,
            tools,
            events,
            seq: Arc::new(AtomicU64::new(0)),
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    async fn emit_lifecycle(
        &self,
        run_id: &str,
        phase: LifecyclePhase,
        message: Option<String>,
    ) -> KelvinResult<()> {
        let event = AgentEvent::lifecycle(self.next_seq(), run_id.to_string(), phase, message);
        self.events.emit(event).await
    }

    async fn emit_assistant(&self, run_id: &str, text: &str, final_chunk: bool) -> KelvinResult<()> {
        let event = AgentEvent::assistant(self.next_seq(), run_id.to_string(), text.to_string(), final_chunk);
        self.events.emit(event).await
    }

    async fn emit_tool(
        &self,
        run_id: &str,
        tool_name: &str,
        phase: ToolPhase,
        summary: Option<String>,
        output: Option<String>,
    ) -> KelvinResult<()> {
        let event = AgentEvent::tool(
            self.next_seq(),
            run_id.to_string(),
            tool_name.to_string(),
            phase,
            summary,
            output,
        );
        self.events.emit(event).await
    }

    async fn run_inner(&self, req: AgentRunRequest) -> KelvinResult<AgentRunResult> {
        if req.prompt.trim().is_empty() {
            return Err(KelvinError::InvalidInput("prompt must not be empty".to_string()));
        }

        let started_at = now_ms();
        self.emit_lifecycle(&req.run_id, LifecyclePhase::Start, None)
            .await?;

        self.session_store
            .upsert_session(SessionDescriptor {
                session_id: req.session_id.clone(),
                session_key: req.session_key.clone(),
                workspace_dir: req.workspace_dir.clone(),
            })
            .await?;

        self.session_store
            .append_message(&req.session_id, SessionMessage::user(req.prompt.clone()))
            .await?;

        let history = self
            .session_store
            .history(&req.session_id)
            .await?
            .into_iter()
            .map(|message| format!("{:?}: {}", message.role, message.content))
            .collect::<Vec<_>>();

        let memory_query = req
            .memory_query
            .clone()
            .unwrap_or_else(|| req.prompt.clone());
        let memory_hits = self
            .memory
            .search(&memory_query, MemorySearchOptions::default())
            .await
            .unwrap_or_default();
        let memory_snippets = memory_hits
            .iter()
            .map(|item| format!("{}#{}-{}: {}", item.path, item.start_line, item.end_line, item.snippet))
            .collect::<Vec<_>>();

        let model_input = ModelInput {
            run_id: req.run_id.clone(),
            session_id: req.session_id.clone(),
            system_prompt: req
                .extra_system_prompt
                .clone()
                .unwrap_or_else(|| "OpenClaw-style Kelvin brain".to_string()),
            user_prompt: req.prompt.clone(),
            memory_snippets,
            history,
        };

        let model_output = self.model.infer(model_input).await?;
        let stop_reason = model_output.stop_reason.clone();
        let tool_calls = model_output.tool_calls;
        let assistant_text = model_output.assistant_text.trim().to_string();

        let mut payloads = Vec::new();
        if !assistant_text.is_empty() && assistant_text != "NO_REPLY" {
            self.emit_assistant(&req.run_id, &assistant_text, true).await?;
            payloads.push(AgentPayload {
                text: assistant_text.clone(),
                is_error: false,
            });
        }

        for tool_call in tool_calls {
            self.emit_tool(
                &req.run_id,
                &tool_call.name,
                ToolPhase::Start,
                Some("tool execution started".to_string()),
                None,
            )
            .await?;

            let Some(tool) = self.tools.get(&tool_call.name) else {
                let summary = format!("unknown tool: {}", tool_call.name);
                self.emit_tool(
                    &req.run_id,
                    &tool_call.name,
                    ToolPhase::Error,
                    Some(summary.clone()),
                    None,
                )
                .await?;
                payloads.push(AgentPayload {
                    text: summary,
                    is_error: true,
                });
                continue;
            };

            let result = tool
                .call(ToolCallInput {
                    run_id: req.run_id.clone(),
                    session_id: req.session_id.clone(),
                    workspace_dir: req.workspace_dir.clone(),
                    arguments: tool_call.arguments.clone(),
                })
                .await?;

            let phase = if result.is_error {
                ToolPhase::Error
            } else {
                ToolPhase::End
            };
            self.emit_tool(
                &req.run_id,
                tool.name(),
                phase,
                Some(result.summary.clone()),
                result.output.clone(),
            )
            .await?;

            self.session_store
                .append_message(
                    &req.session_id,
                    SessionMessage::tool(
                        result.summary.clone(),
                        json!({
                            "tool": tool.name(),
                            "is_error": result.is_error,
                            "output": result.output,
                        }),
                    ),
                )
                .await?;

            if let Some(visible_text) = result.visible_text {
                payloads.push(AgentPayload {
                    text: visible_text,
                    is_error: result.is_error,
                });
            }
        }

        if !assistant_text.is_empty() {
            self.session_store
                .append_message(&req.session_id, SessionMessage::assistant(assistant_text))
                .await?;
        }

        self.emit_lifecycle(&req.run_id, LifecyclePhase::End, None)
            .await?;

        let duration_ms = now_ms().saturating_sub(started_at);
        Ok(AgentRunResult {
            payloads,
            meta: AgentRunMeta {
                duration_ms,
                provider: self.model.provider_name().to_string(),
                model: self.model.model_name().to_string(),
                stop_reason,
                error: None,
            },
        })
    }
}

#[async_trait]
impl Brain for OpenClawBrain {
    async fn run(&self, req: AgentRunRequest) -> KelvinResult<AgentRunResult> {
        let run_id = req.run_id.clone();
        let result = match req.timeout_ms {
            Some(timeout_ms) => {
                match time::timeout(Duration::from_millis(timeout_ms), self.run_inner(req)).await {
                    Ok(inner_result) => inner_result,
                    Err(_) => {
                        Err(KelvinError::Timeout(format!(
                            "agent run exceeded timeout of {timeout_ms}ms"
                        )))
                    }
                }
            }
            None => self.run_inner(req).await,
        };

        if let Err(err) = &result {
            let _ = self
                .emit_lifecycle(&run_id, LifecyclePhase::Error, Some(err.to_string()))
                .await;
        }

        result
    }
}
