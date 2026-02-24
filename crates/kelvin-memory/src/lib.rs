pub mod factory;
pub mod fallback;
pub mod in_memory;
pub mod markdown;

pub use factory::{MemoryBackendKind, MemoryFactory};
pub use fallback::FallbackMemoryManager;
pub use in_memory::{InMemoryDocument, InMemoryVectorMemoryManager};
pub use markdown::MarkdownMemoryManager;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;

    use kelvin_core::{
        KelvinError, KelvinResult, MemoryEmbeddingProbeResult, MemoryProviderStatus,
        MemoryReadParams, MemoryReadResult, MemorySearchManager, MemorySearchOptions,
        MemorySearchResult, MemorySyncParams,
    };

    use crate::{
        FallbackMemoryManager, InMemoryDocument, InMemoryVectorMemoryManager, MemoryBackendKind,
        MemoryFactory,
    };

    struct FailingMemoryManager;

    #[async_trait]
    impl MemorySearchManager for FailingMemoryManager {
        async fn search(
            &self,
            _query: &str,
            _opts: MemorySearchOptions,
        ) -> KelvinResult<Vec<MemorySearchResult>> {
            Err(KelvinError::Backend(
                "primary backend unavailable".to_string(),
            ))
        }

        async fn read_file(&self, _params: MemoryReadParams) -> KelvinResult<MemoryReadResult> {
            Err(KelvinError::Backend(
                "primary backend unavailable".to_string(),
            ))
        }

        fn status(&self) -> MemoryProviderStatus {
            MemoryProviderStatus {
                backend: "builtin".to_string(),
                provider: "failing".to_string(),
                model: None,
                requested_provider: Some("failing".to_string()),
                files: None,
                chunks: None,
                dirty: true,
                fallback: None,
                custom: serde_json::json!({}),
            }
        }

        async fn sync(&self, _params: Option<MemorySyncParams>) -> KelvinResult<()> {
            Err(KelvinError::Backend(
                "primary backend unavailable".to_string(),
            ))
        }

        async fn probe_embedding_availability(&self) -> KelvinResult<MemoryEmbeddingProbeResult> {
            Ok(MemoryEmbeddingProbeResult {
                ok: false,
                error: Some("primary backend unavailable".to_string()),
            })
        }

        async fn probe_vector_availability(&self) -> KelvinResult<bool> {
            Ok(false)
        }
    }

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir() -> PathBuf {
        let suffix = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "kelvin-memory-test-{}-{suffix}",
            kelvin_core::now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test temp dir");
        dir
    }

    #[tokio::test]
    async fn markdown_manager_gracefully_reads_missing_file() {
        let temp_dir = unique_temp_dir();
        let manager = crate::MarkdownMemoryManager::new(&temp_dir);

        let result = manager
            .read_file(MemoryReadParams {
                rel_path: "memory/2099-01-01.md".to_string(),
                from: None,
                lines: None,
            })
            .await
            .expect("read_file");

        assert_eq!(result.path, "memory/2099-01-01.md");
        assert!(result.text.is_empty());
    }

    #[tokio::test]
    async fn fallback_manager_uses_secondary_after_primary_failure() {
        let fallback = Arc::new(InMemoryVectorMemoryManager::new(vec![InMemoryDocument {
            path: "MEMORY.md".to_string(),
            text: "router uses vlan10".to_string(),
            source: kelvin_core::MemorySource::Memory,
        }]));
        let manager = FallbackMemoryManager::new(Arc::new(FailingMemoryManager), fallback);

        let results = manager
            .search("router", MemorySearchOptions::default())
            .await
            .expect("search with fallback");

        assert_eq!(results.len(), 1);
        let status = manager.status();
        assert!(status.fallback.is_some());
    }

    #[tokio::test]
    async fn factory_builds_swappable_backends() {
        let temp_dir = unique_temp_dir();
        fs::create_dir_all(temp_dir.join("memory")).expect("create memory dir");
        fs::write(
            temp_dir.join("memory").join("2026-02-24.md"),
            "configured omada router on vlan10",
        )
        .expect("write memory file");

        for kind in [
            MemoryBackendKind::Markdown,
            MemoryBackendKind::InMemoryVector,
            MemoryBackendKind::InMemoryWithMarkdownFallback,
        ] {
            let manager = MemoryFactory::build(&temp_dir, kind);
            let hits = manager
                .search("router", MemorySearchOptions::default())
                .await
                .expect("search by backend");
            assert!(!hits.is_empty());
        }
    }

    #[tokio::test]
    async fn markdown_search_tie_breaker_is_deterministic() {
        let temp_dir = unique_temp_dir();
        fs::create_dir_all(temp_dir.join("memory")).expect("create memory dir");
        fs::write(temp_dir.join("memory").join("b.md"), "router").expect("write b");
        fs::write(temp_dir.join("memory").join("a.md"), "router").expect("write a");

        let manager = crate::MarkdownMemoryManager::new(&temp_dir);
        let hits = manager
            .search("router", MemorySearchOptions::default())
            .await
            .expect("search");

        let paths = hits.iter().map(|hit| hit.path.as_str()).collect::<Vec<_>>();
        assert_eq!(paths, vec!["memory/a.md", "memory/b.md"]);
    }

    #[tokio::test]
    async fn in_memory_search_tie_breaker_is_deterministic() {
        let manager = InMemoryVectorMemoryManager::new(vec![
            InMemoryDocument {
                path: "z.md".to_string(),
                text: "router".to_string(),
                source: kelvin_core::MemorySource::Memory,
            },
            InMemoryDocument {
                path: "a.md".to_string(),
                text: "router".to_string(),
                source: kelvin_core::MemorySource::Memory,
            },
        ]);

        let hits = manager
            .search("router", MemorySearchOptions::default())
            .await
            .expect("search");
        let paths = hits.iter().map(|hit| hit.path.as_str()).collect::<Vec<_>>();
        assert_eq!(paths, vec!["a.md", "z.md"]);
    }
}
