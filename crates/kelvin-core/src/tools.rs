use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::KelvinResult;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallInput {
    pub run_id: String,
    pub session_id: String,
    pub workspace_dir: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallResult {
    pub summary: String,
    pub output: Option<String>,
    pub visible_text: Option<String>,
    pub is_error: bool,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    async fn call(&self, input: ToolCallInput) -> KelvinResult<ToolCallResult>;
}

pub trait ToolRegistry: Send + Sync {
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>>;

    fn names(&self) -> Vec<String>;
}
