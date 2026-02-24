use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{KelvinResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunRequest {
    pub run_id: String,
    pub session_id: String,
    pub session_key: String,
    pub workspace_dir: String,
    pub prompt: String,
    pub extra_system_prompt: Option<String>,
    pub timeout_ms: Option<u64>,
    pub memory_query: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentPayload {
    pub text: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRunMeta {
    pub duration_ms: u128,
    pub provider: String,
    pub model: String,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRunResult {
    pub payloads: Vec<AgentPayload>,
    pub meta: AgentRunMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaitStatus {
    Ok,
    Error,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWaitResult {
    pub status: WaitStatus,
    pub started_at_ms: u128,
    pub ended_at_ms: Option<u128>,
    pub error: Option<String>,
}

#[async_trait]
pub trait Brain: Send + Sync {
    async fn run(&self, req: AgentRunRequest) -> KelvinResult<AgentRunResult>;
}
