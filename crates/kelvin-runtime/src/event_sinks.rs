use async_trait::async_trait;
use tokio::sync::RwLock;

use kelvin_core::{AgentEvent, EventSink, KelvinResult};

#[derive(Debug, Default)]
pub struct InMemoryEventSink {
    events: RwLock<Vec<AgentEvent>>,
}

impl InMemoryEventSink {
    pub async fn all(&self) -> Vec<AgentEvent> {
        self.events.read().await.clone()
    }
}

#[async_trait]
impl EventSink for InMemoryEventSink {
    async fn emit(&self, event: AgentEvent) -> KelvinResult<()> {
        self.events.write().await.push(event);
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct StdoutEventSink;

#[async_trait]
impl EventSink for StdoutEventSink {
    async fn emit(&self, event: AgentEvent) -> KelvinResult<()> {
        println!("{}", serde_json::to_string(&event).unwrap_or_default());
        Ok(())
    }
}
