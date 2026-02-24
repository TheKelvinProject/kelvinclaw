use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};

use async_trait::async_trait;
use serde_json::{json, Value};

use kelvin_core::{
    KelvinResult, MemoryEmbeddingProbeResult, MemoryFallbackStatus, MemoryProviderStatus,
    MemoryReadParams, MemoryReadResult, MemorySearchManager, MemorySearchOptions,
    MemorySearchResult, MemorySyncParams,
};

pub struct FallbackMemoryManager {
    primary: Arc<dyn MemorySearchManager>,
    fallback: Arc<dyn MemorySearchManager>,
    primary_failed: AtomicBool,
    last_error: RwLock<Option<String>>,
}

impl FallbackMemoryManager {
    pub fn new(
        primary: Arc<dyn MemorySearchManager>,
        fallback: Arc<dyn MemorySearchManager>,
    ) -> Self {
        Self {
            primary,
            fallback,
            primary_failed: AtomicBool::new(false),
            last_error: RwLock::new(None),
        }
    }

    fn set_failed(&self, reason: String) {
        self.primary_failed.store(true, Ordering::SeqCst);
        let mut last_error = self
            .last_error
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *last_error = Some(reason);
    }

    fn is_failed(&self) -> bool {
        self.primary_failed.load(Ordering::SeqCst)
    }

    fn last_error(&self) -> Option<String> {
        self.last_error
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn merged_status(&self, mut base: MemoryProviderStatus) -> MemoryProviderStatus {
        if self.is_failed() {
            base.fallback = Some(MemoryFallbackStatus {
                from: base.provider.clone(),
                reason: self.last_error(),
            });
            base.custom = merge_json(
                base.custom,
                json!({
                    "fallback": {
                        "enabled": true,
                        "reason": self.last_error(),
                    }
                }),
            );
        }
        base
    }
}

fn merge_json(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Object(mut base_map), Value::Object(overlay_map)) => {
            for (key, value) in overlay_map {
                base_map.insert(key, value);
            }
            Value::Object(base_map)
        }
        (_, overlay_value) => overlay_value,
    }
}

#[async_trait]
impl MemorySearchManager for FallbackMemoryManager {
    async fn search(
        &self,
        query: &str,
        opts: MemorySearchOptions,
    ) -> KelvinResult<Vec<MemorySearchResult>> {
        if !self.is_failed() {
            match self.primary.search(query, opts.clone()).await {
                Ok(results) => return Ok(results),
                Err(err) => self.set_failed(err.to_string()),
            }
        }
        self.fallback.search(query, opts).await
    }

    async fn read_file(&self, params: MemoryReadParams) -> KelvinResult<MemoryReadResult> {
        if !self.is_failed() {
            match self.primary.read_file(params.clone()).await {
                Ok(result) => return Ok(result),
                Err(err) => self.set_failed(err.to_string()),
            }
        }
        self.fallback.read_file(params).await
    }

    fn status(&self) -> MemoryProviderStatus {
        if self.is_failed() {
            self.merged_status(self.fallback.status())
        } else {
            self.merged_status(self.primary.status())
        }
    }

    async fn sync(&self, params: Option<MemorySyncParams>) -> KelvinResult<()> {
        if !self.is_failed() {
            match self.primary.sync(params.clone()).await {
                Ok(()) => return Ok(()),
                Err(err) => self.set_failed(err.to_string()),
            }
        }
        self.fallback.sync(params).await
    }

    async fn probe_embedding_availability(&self) -> KelvinResult<MemoryEmbeddingProbeResult> {
        if !self.is_failed() {
            match self.primary.probe_embedding_availability().await {
                Ok(result) => return Ok(result),
                Err(err) => self.set_failed(err.to_string()),
            }
        }
        self.fallback.probe_embedding_availability().await
    }

    async fn probe_vector_availability(&self) -> KelvinResult<bool> {
        if !self.is_failed() {
            match self.primary.probe_vector_availability().await {
                Ok(result) => return Ok(result),
                Err(err) => self.set_failed(err.to_string()),
            }
        }
        self.fallback.probe_vector_availability().await
    }
}
