use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::KelvinResult;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionDescriptor {
    pub session_id: String,
    pub session_key: String,
    pub workspace_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMessage {
    pub role: SessionRole,
    pub content: String,
    pub metadata: Value,
}

impl SessionMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: SessionRole::User,
            content: content.into(),
            metadata: Value::Object(Default::default()),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: SessionRole::Assistant,
            content: content.into(),
            metadata: Value::Object(Default::default()),
        }
    }

    pub fn tool(content: impl Into<String>, metadata: Value) -> Self {
        Self {
            role: SessionRole::Tool,
            content: content.into(),
            metadata,
        }
    }
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn upsert_session(&self, session: SessionDescriptor) -> KelvinResult<()>;

    async fn get_session(&self, session_id: &str) -> KelvinResult<Option<SessionDescriptor>>;

    async fn append_message(&self, session_id: &str, message: SessionMessage) -> KelvinResult<()>;

    async fn history(&self, session_id: &str) -> KelvinResult<Vec<SessionMessage>>;
}
