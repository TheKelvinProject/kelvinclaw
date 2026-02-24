use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::KelvinResult;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    Memory,
    Sessions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemorySearchResult {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f32,
    pub snippet: String,
    pub source: MemorySource,
    pub citation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryEmbeddingProbeResult {
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySyncProgressUpdate {
    pub completed: usize,
    pub total: usize,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryFallbackStatus {
    pub from: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryProviderStatus {
    pub backend: String,
    pub provider: String,
    pub model: Option<String>,
    pub requested_provider: Option<String>,
    pub files: Option<usize>,
    pub chunks: Option<usize>,
    pub dirty: bool,
    pub fallback: Option<MemoryFallbackStatus>,
    pub custom: Value,
}

impl Default for MemoryProviderStatus {
    fn default() -> Self {
        Self {
            backend: "builtin".to_string(),
            provider: "unknown".to_string(),
            model: None,
            requested_provider: None,
            files: None,
            chunks: None,
            dirty: false,
            fallback: None,
            custom: Value::Object(Default::default()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySearchOptions {
    pub max_results: usize,
    pub min_score_milli: u16,
    pub session_key: Option<String>,
}

impl Default for MemorySearchOptions {
    fn default() -> Self {
        Self {
            max_results: 6,
            min_score_milli: 0,
            session_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryReadParams {
    pub rel_path: String,
    pub from: Option<usize>,
    pub lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryReadResult {
    pub text: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySyncParams {
    pub reason: Option<String>,
    pub force: bool,
}

#[async_trait]
pub trait MemorySearchManager: Send + Sync {
    async fn search(
        &self,
        query: &str,
        opts: MemorySearchOptions,
    ) -> KelvinResult<Vec<MemorySearchResult>>;

    async fn read_file(&self, params: MemoryReadParams) -> KelvinResult<MemoryReadResult>;

    fn status(&self) -> MemoryProviderStatus;

    async fn sync(&self, _params: Option<MemorySyncParams>) -> KelvinResult<()> {
        Ok(())
    }

    async fn probe_embedding_availability(&self) -> KelvinResult<MemoryEmbeddingProbeResult>;

    async fn probe_vector_availability(&self) -> KelvinResult<bool>;
}
