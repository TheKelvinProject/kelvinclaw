use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use async_trait::async_trait;
use serde_json::json;
use walkdir::WalkDir;

use kelvin_core::{
    KelvinError, KelvinResult, MemoryEmbeddingProbeResult, MemoryProviderStatus, MemoryReadParams,
    MemoryReadResult, MemorySearchManager, MemorySearchOptions, MemorySearchResult, MemorySource,
    MemorySyncParams,
};

#[derive(Debug)]
pub struct MarkdownMemoryManager {
    workspace_dir: PathBuf,
    status: RwLock<MemoryProviderStatus>,
}

impl MarkdownMemoryManager {
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            status: RwLock::new(MemoryProviderStatus {
                backend: "builtin".to_string(),
                provider: "markdown".to_string(),
                model: None,
                requested_provider: Some("markdown".to_string()),
                files: Some(0),
                chunks: Some(0),
                dirty: true,
                fallback: None,
                custom: json!({"source_of_truth": "workspace_markdown"}),
            }),
        }
    }

    fn collect_memory_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();

        let long_term = self.workspace_dir.join("MEMORY.md");
        if long_term.is_file() {
            files.push(long_term);
        }

        let daily_dir = self.workspace_dir.join("memory");
        if daily_dir.is_dir() {
            for entry in WalkDir::new(daily_dir)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if path
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
                {
                    files.push(path.to_path_buf());
                }
            }
        }

        files
    }

    fn to_relative_path(&self, abs: &Path) -> String {
        abs.strip_prefix(&self.workspace_dir)
            .unwrap_or(abs)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn tokenize(input: &str) -> Vec<String> {
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

    fn score_line(line: &str, tokens: &[String]) -> usize {
        let normalized = line.to_lowercase();
        tokens
            .iter()
            .filter(|token| normalized.contains(token.as_str()))
            .count()
    }

    fn build_snippet(lines: &[&str], idx: usize) -> (usize, usize, String) {
        let start = idx.saturating_sub(1);
        let end = (idx + 1).min(lines.len().saturating_sub(1));
        let snippet = lines[start..=end].join("\n");
        (start + 1, end + 1, snippet)
    }

    fn validate_rel_path(rel_path: &str) -> KelvinResult<()> {
        if rel_path.is_empty() {
            return Err(KelvinError::InvalidInput(
                "memory_get path must not be empty".to_string(),
            ));
        }
        if rel_path.contains("..") {
            return Err(KelvinError::InvalidInput(
                "memory_get path traversal is not allowed".to_string(),
            ));
        }
        let normalized = rel_path.replace('\\', "/");
        let is_memory_root = normalized == "MEMORY.md";
        let is_daily = normalized.starts_with("memory/");
        if !is_memory_root && !is_daily {
            return Err(KelvinError::InvalidInput(format!(
                "memory_get path is out of scope: {normalized}"
            )));
        }
        if Path::new(&normalized).is_absolute() {
            return Err(KelvinError::InvalidInput(
                "memory_get absolute paths are not allowed".to_string(),
            ));
        }
        Ok(())
    }

    fn slice_lines(text: &str, from: Option<usize>, lines: Option<usize>) -> String {
        let raw_lines: Vec<&str> = text.lines().collect();
        if raw_lines.is_empty() {
            return String::new();
        }
        let start = from.unwrap_or(1).saturating_sub(1);
        if start >= raw_lines.len() {
            return String::new();
        }
        let count = lines.unwrap_or(raw_lines.len().saturating_sub(start));
        let end = (start + count).min(raw_lines.len());
        raw_lines[start..end].join("\n")
    }
}

#[async_trait]
impl MemorySearchManager for MarkdownMemoryManager {
    async fn search(
        &self,
        query: &str,
        opts: MemorySearchOptions,
    ) -> KelvinResult<Vec<MemorySearchResult>> {
        let cleaned = query.trim();
        if cleaned.is_empty() {
            return Ok(Vec::new());
        }
        let tokens = Self::tokenize(cleaned);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        let mut files_seen = 0usize;
        let mut chunks_seen = 0usize;
        let mut results = Vec::new();

        for file in self.collect_memory_files() {
            let text = match fs::read_to_string(&file) {
                Ok(value) => value,
                Err(_) => continue,
            };
            files_seen += 1;

            let lines: Vec<&str> = text.lines().collect();
            chunks_seen += lines.len();

            for (idx, line) in lines.iter().enumerate() {
                let matched = Self::score_line(line, &tokens);
                if matched == 0 {
                    continue;
                }
                let score = (matched as f32) / (tokens.len() as f32);
                if (score * 1000.0) < opts.min_score_milli as f32 {
                    continue;
                }
                let (start_line, end_line, snippet) = Self::build_snippet(&lines, idx);
                let rel_path = self.to_relative_path(&file);
                results.push(MemorySearchResult {
                    path: rel_path.clone(),
                    start_line,
                    end_line,
                    score,
                    snippet,
                    source: MemorySource::Memory,
                    citation: Some(format!("{rel_path}#{start_line}")),
                });
            }
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

        {
            let mut status = self
                .status
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            status.files = Some(files_seen);
            status.chunks = Some(chunks_seen);
            status.dirty = false;
        }

        Ok(results)
    }

    async fn read_file(&self, params: MemoryReadParams) -> KelvinResult<MemoryReadResult> {
        let rel = params.rel_path.trim().replace('\\', "/");
        Self::validate_rel_path(&rel)?;

        let target = self.workspace_dir.join(&rel);
        if !target.is_file() {
            return Ok(MemoryReadResult {
                text: String::new(),
                path: rel,
            });
        }

        let text = fs::read_to_string(target)?;
        let sliced = Self::slice_lines(&text, params.from, params.lines);
        Ok(MemoryReadResult {
            text: sliced,
            path: rel,
        })
    }

    fn status(&self) -> MemoryProviderStatus {
        self.status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn sync(&self, _params: Option<MemorySyncParams>) -> KelvinResult<()> {
        let mut status = self
            .status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status.dirty = false;
        Ok(())
    }

    async fn probe_embedding_availability(&self) -> KelvinResult<MemoryEmbeddingProbeResult> {
        Ok(MemoryEmbeddingProbeResult {
            ok: false,
            error: Some("markdown backend has no embedding provider".to_string()),
        })
    }

    async fn probe_vector_availability(&self) -> KelvinResult<bool> {
        Ok(false)
    }
}
