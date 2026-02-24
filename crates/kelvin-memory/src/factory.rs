use std::path::Path;
use std::sync::Arc;

use kelvin_core::MemorySearchManager;

use crate::{FallbackMemoryManager, InMemoryVectorMemoryManager, MarkdownMemoryManager};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryBackendKind {
    Markdown,
    InMemoryVector,
    InMemoryWithMarkdownFallback,
}

impl MemoryBackendKind {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "markdown" => Self::Markdown,
            "in-memory" | "in_memory" | "vector" => Self::InMemoryVector,
            "fallback" | "in-memory-fallback" | "in_memory_fallback" => {
                Self::InMemoryWithMarkdownFallback
            }
            _ => Self::Markdown,
        }
    }
}

#[derive(Debug, Default)]
pub struct MemoryFactory;

impl MemoryFactory {
    pub fn build(workspace_dir: impl AsRef<Path>, kind: MemoryBackendKind) -> Arc<dyn MemorySearchManager> {
        let workspace_dir = workspace_dir.as_ref();

        match kind {
            MemoryBackendKind::Markdown => Arc::new(MarkdownMemoryManager::new(workspace_dir)),
            MemoryBackendKind::InMemoryVector => {
                Arc::new(InMemoryVectorMemoryManager::from_workspace(workspace_dir))
            }
            MemoryBackendKind::InMemoryWithMarkdownFallback => {
                let primary = Arc::new(InMemoryVectorMemoryManager::from_workspace(workspace_dir));
                let fallback = Arc::new(MarkdownMemoryManager::new(workspace_dir));
                Arc::new(FallbackMemoryManager::new(primary, fallback))
            }
        }
    }
}
