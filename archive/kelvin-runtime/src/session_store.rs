use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;

use kelvin_core::{
    KelvinError, KelvinResult, SessionDescriptor, SessionMessage, SessionStore,
};

#[derive(Debug, Clone)]
struct SessionRecord {
    descriptor: SessionDescriptor,
    messages: Vec<SessionMessage>,
}

#[derive(Debug, Default)]
pub struct InMemorySessionStore {
    sessions: RwLock<HashMap<String, SessionRecord>>,
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn upsert_session(&self, session: SessionDescriptor) -> KelvinResult<()> {
        let mut guard = self.sessions.write().await;
        let record = guard.entry(session.session_id.clone()).or_insert(SessionRecord {
            descriptor: session.clone(),
            messages: Vec::new(),
        });
        record.descriptor = session;
        Ok(())
    }

    async fn get_session(&self, session_id: &str) -> KelvinResult<Option<SessionDescriptor>> {
        let guard = self.sessions.read().await;
        Ok(guard.get(session_id).map(|record| record.descriptor.clone()))
    }

    async fn append_message(&self, session_id: &str, message: SessionMessage) -> KelvinResult<()> {
        let mut guard = self.sessions.write().await;
        let Some(record) = guard.get_mut(session_id) else {
            return Err(KelvinError::NotFound(format!(
                "session does not exist: {session_id}"
            )));
        };
        record.messages.push(message);
        Ok(())
    }

    async fn history(&self, session_id: &str) -> KelvinResult<Vec<SessionMessage>> {
        let guard = self.sessions.read().await;
        Ok(guard
            .get(session_id)
            .map(|record| record.messages.clone())
            .unwrap_or_default())
    }
}
