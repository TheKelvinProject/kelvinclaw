use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::json;
use walkdir::WalkDir;

use kelvin_core::{
    KelvinResult, MemoryEmbeddingProbeResult, MemoryProviderStatus, MemoryReadParams,
    MemoryReadResult, MemorySearchManager, MemorySearchOptions, MemorySearchResult, MemorySource,
    MemorySyncParams,
};

#[derive(Debug, Clone)]
pub struct InMemoryDocument {
    pub path: String,
    pub text: String,
    pub source: MemorySource,
}

#[derive(Debug, Clone)]
pub struct InMemoryVectorMemoryManager {
    docs: Arc<RwLock<Vec<InMemoryDocument>>>,
    status: Arc<RwLock<MemoryProviderStatus>>,
}

impl InMemoryVectorMemoryManager {
    pub fn new(docs: Vec<InMemoryDocument>) -> Self {
        let files = docs.len();
        let chunks = docs.iter().map(|doc| doc.text.lines().count()).sum();
        Self {
            docs: Arc::new(RwLock::new(docs)),
            status: Arc::new(RwLock::new(MemoryProviderStatus {
                backend: "builtin".to_string(),
                provider: "in_memory_vector".to_string(),
                model: Some("token-overlap-v1".to_string()),
                requested_provider: Some("in_memory_vector".to_string()),
                files: Some(files),
                chunks: Some(chunks),
                dirty: false,
                fallback: None,
                custom: json!({"index": "volatile"}),
            })),
        }
    }

    pub fn from_workspace(workspace_dir: impl AsRef<Path>) -> Self {
        let workspace_dir = workspace_dir.as_ref();
        let mut docs = Vec::new();

        let memory_root = workspace_dir.join("MEMORY.md");
        if let Ok(text) = fs::read_to_string(memory_root) {
            docs.push(InMemoryDocument {
                path: "MEMORY.md".to_string(),
                text,
                source: MemorySource::Memory,
            });
        }

        let daily_dir = workspace_dir.join("memory");
        if daily_dir.is_dir() {
            for entry in WalkDir::new(daily_dir)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let file_path = entry.path();
                if !file_path
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
                {
                    continue;
                }
                let Ok(text) = fs::read_to_string(file_path) else {
                    continue;
                };
                let rel = file_path
                    .strip_prefix(workspace_dir)
                    .unwrap_or(file_path)
                    .to_string_lossy()
                    .replace('\\', "/");
                docs.push(InMemoryDocument {
                    path: rel,
                    text,
                    source: MemorySource::Memory,
                });
            }
        }

        Self::new(docs)
    }

    pub fn push_document(&self, doc: InMemoryDocument) {
        let mut docs = self
            .docs
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        docs.push(doc);
    }

    fn tokenize(input: &str) -> HashSet<String> {
        input
            .split_whitespace()
            .map(|token| {
                token
                    .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
                    .to_lowercase()
            })
            .filter(|token| !token.is_empty())
            .collect()
    }

    fn compute_similarity(query_tokens: &HashSet<String>, text_tokens: &HashSet<String>) -> f32 {
        if query_tokens.is_empty() || text_tokens.is_empty() {
            return 0.0;
        }
        let overlap = query_tokens
            .iter()
            .filter(|token| text_tokens.contains(*token))
            .count();
        overlap as f32 / query_tokens.len().max(1) as f32
    }

    fn build_snippet(text: &str, query_tokens: &HashSet<String>) -> (usize, usize, String) {
        let lines: Vec<&str> = text.lines().collect();
        if lines.is_empty() {
            return (1, 1, String::new());
        }

        let mut first_hit: Option<usize> = None;
        for (idx, line) in lines.iter().enumerate() {
            let normalized = line.to_lowercase();
            if query_tokens
                .iter()
                .any(|token| normalized.contains(token.as_str()))
            {
                first_hit = Some(idx);
                break;
            }
        }

        let idx = first_hit.unwrap_or(0);
        let start = idx.saturating_sub(1);
        let end = (idx + 1).min(lines.len().saturating_sub(1));
        (start + 1, end + 1, lines[start..=end].join("\n"))
    }

    fn build_doc_map<'a>(docs: &'a [InMemoryDocument]) -> HashMap<&'a str, &'a InMemoryDocument> {
        docs.iter().map(|doc| (doc.path.as_str(), doc)).collect()
    }
}

#[async_trait]
impl MemorySearchManager for InMemoryVectorMemoryManager {
    async fn search(
        &self,
        query: &str,
        opts: MemorySearchOptions,
    ) -> KelvinResult<Vec<MemorySearchResult>> {
        let cleaned = query.trim();
        if cleaned.is_empty() {
            return Ok(Vec::new());
        }
        let query_tokens = Self::tokenize(cleaned);
        if query_tokens.is_empty() {
            return Ok(Vec::new());
        }

        let docs = self
            .docs
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        let mut results = Vec::new();
        for doc in docs {
            let text_tokens = Self::tokenize(&doc.text);
            let score = Self::compute_similarity(&query_tokens, &text_tokens);
            if score <= 0.0 {
                continue;
            }
            if (score * 1000.0) < opts.min_score_milli as f32 {
                continue;
            }
            let (start_line, end_line, snippet) = Self::build_snippet(&doc.text, &query_tokens);
            results.push(MemorySearchResult {
                path: doc.path.clone(),
                start_line,
                end_line,
                score,
                snippet,
                source: doc.source,
                citation: Some(format!("{}#{}", doc.path, start_line)),
            });
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.start_line.cmp(&b.start_line))
                .then_with(|| a.end_line.cmp(&b.end_line))
        });
        results.truncate(opts.max_results.max(1));

        Ok(results)
    }

    async fn read_file(&self, params: MemoryReadParams) -> KelvinResult<MemoryReadResult> {
        let rel = params.rel_path.trim().replace('\\', "/");
        let docs_guard = self
            .docs
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let doc_map = Self::build_doc_map(&docs_guard);

        let Some(doc) = doc_map.get(rel.as_str()) else {
            return Ok(MemoryReadResult {
                text: String::new(),
                path: rel,
            });
        };

        let raw_lines: Vec<&str> = doc.text.lines().collect();
        if raw_lines.is_empty() {
            return Ok(MemoryReadResult {
                text: String::new(),
                path: rel,
            });
        }

        let start = params.from.unwrap_or(1).saturating_sub(1);
        if start >= raw_lines.len() {
            return Ok(MemoryReadResult {
                text: String::new(),
                path: rel,
            });
        }
        let count = params
            .lines
            .unwrap_or(raw_lines.len().saturating_sub(start));
        let end = (start + count).min(raw_lines.len());
        let text = raw_lines[start..end].join("\n");

        Ok(MemoryReadResult { text, path: rel })
    }

    fn status(&self) -> MemoryProviderStatus {
        self.status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn sync(&self, _params: Option<MemorySyncParams>) -> KelvinResult<()> {
        let docs = self
            .docs
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut status = self
            .status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status.files = Some(docs.len());
        status.chunks = Some(docs.iter().map(|doc| doc.text.lines().count()).sum());
        status.dirty = false;
        Ok(())
    }

    async fn probe_embedding_availability(&self) -> KelvinResult<MemoryEmbeddingProbeResult> {
        Ok(MemoryEmbeddingProbeResult {
            ok: true,
            error: None,
        })
    }

    async fn probe_vector_availability(&self) -> KelvinResult<bool> {
        Ok(true)
    }
}
