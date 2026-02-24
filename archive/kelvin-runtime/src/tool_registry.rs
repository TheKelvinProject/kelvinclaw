use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use kelvin_core::{KelvinResult, Tool, ToolCallInput, ToolCallResult, ToolRegistry};

#[derive(Default)]
pub struct HashMapToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
}

impl HashMapToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&self, tool: T)
    where
        T: Tool + 'static,
    {
        let name = tool.name().to_string();
        let mut guard = self
            .tools
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(name, Arc::new(tool));
    }
}

impl ToolRegistry for HashMapToolRegistry {
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .cloned()
    }

    fn names(&self) -> Vec<String> {
        self.tools
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct TimeTool;

#[async_trait]
impl Tool for TimeTool {
    fn name(&self) -> &str {
        "time"
    }

    async fn call(&self, _input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        Ok(ToolCallResult {
            summary: "timestamp generated".to_string(),
            output: Some(now_ms.to_string()),
            visible_text: Some(format!("Current unix epoch millis: {now_ms}")),
            is_error: false,
        })
    }
}

#[derive(Debug, Clone)]
pub struct StaticTextTool {
    name: String,
    text: String,
}

impl StaticTextTool {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
        }
    }
}

#[async_trait]
impl Tool for StaticTextTool {
    fn name(&self) -> &str {
        &self.name
    }

    async fn call(&self, _input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        Ok(ToolCallResult {
            summary: format!("{} returned static text", self.name),
            output: Some(self.text.clone()),
            visible_text: Some(self.text.clone()),
            is_error: false,
        })
    }
}
