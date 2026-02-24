use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::KelvinResult;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInput {
    pub run_id: String,
    pub session_id: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub memory_snippets: Vec<String>,
    pub history: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelOutput {
    pub assistant_text: String,
    pub stop_reason: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<ModelUsage>,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn provider_name(&self) -> &str;

    fn model_name(&self) -> &str;

    async fn infer(&self, input: ModelInput) -> KelvinResult<ModelOutput>;
}
