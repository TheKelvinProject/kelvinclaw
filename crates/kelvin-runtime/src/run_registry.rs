use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify, RwLock};
use tokio::time;

use kelvin_core::{
    now_ms, AgentRunResult, AgentWaitResult, KelvinError, KelvinResult, WaitStatus,
};

#[derive(Debug, Clone)]
enum RunStatus {
    Accepted,
    Running,
    Completed(AgentRunResult),
    Failed(String),
}

#[derive(Debug)]
struct RunRecord {
    started_at_ms: u128,
    ended_at_ms: Option<u128>,
    status: RunStatus,
    notify: Arc<Notify>,
}

#[derive(Debug, Clone)]
pub enum StoredRunResult {
    Completed(AgentRunResult),
    Failed(String),
}

#[derive(Debug, Default)]
pub struct RunRegistry {
    runs: RwLock<HashMap<String, Arc<Mutex<RunRecord>>>>,
}

impl RunRegistry {
    pub async fn register(&self, run_id: &str) -> KelvinResult<()> {
        let mut guard = self.runs.write().await;
        if guard.contains_key(run_id) {
            return Err(KelvinError::InvalidInput(format!(
                "run id already exists: {run_id}"
            )));
        }

        guard.insert(
            run_id.to_string(),
            Arc::new(Mutex::new(RunRecord {
                started_at_ms: now_ms(),
                ended_at_ms: None,
                status: RunStatus::Accepted,
                notify: Arc::new(Notify::new()),
            })),
        );
        Ok(())
    }

    async fn get_record(&self, run_id: &str) -> KelvinResult<Arc<Mutex<RunRecord>>> {
        let guard = self.runs.read().await;
        guard
            .get(run_id)
            .cloned()
            .ok_or_else(|| KelvinError::NotFound(format!("unknown run id: {run_id}")))
    }

    pub async fn mark_running(&self, run_id: &str) -> KelvinResult<()> {
        let record = self.get_record(run_id).await?;
        let mut guard = record.lock().await;
        guard.status = RunStatus::Running;
        Ok(())
    }

    pub async fn mark_completed(&self, run_id: &str, result: AgentRunResult) -> KelvinResult<()> {
        let record = self.get_record(run_id).await?;
        let notify = {
            let mut guard = record.lock().await;
            guard.ended_at_ms = Some(now_ms());
            guard.status = RunStatus::Completed(result);
            guard.notify.clone()
        };
        notify.notify_waiters();
        Ok(())
    }

    pub async fn mark_failed(&self, run_id: &str, error: String) -> KelvinResult<()> {
        let record = self.get_record(run_id).await?;
        let notify = {
            let mut guard = record.lock().await;
            guard.ended_at_ms = Some(now_ms());
            guard.status = RunStatus::Failed(error);
            guard.notify.clone()
        };
        notify.notify_waiters();
        Ok(())
    }

    pub async fn wait(&self, run_id: &str, timeout_ms: u64) -> KelvinResult<AgentWaitResult> {
        let record = self.get_record(run_id).await?;
        let timeout = Duration::from_millis(timeout_ms.max(1));

        loop {
            let (started_at, status, ended_at, notify) = {
                let guard = record.lock().await;
                (
                    guard.started_at_ms,
                    guard.status.clone(),
                    guard.ended_at_ms,
                    guard.notify.clone(),
                )
            };

            match status {
                RunStatus::Completed(_) => {
                    return Ok(AgentWaitResult {
                        status: WaitStatus::Ok,
                        started_at_ms: started_at,
                        ended_at_ms: ended_at,
                        error: None,
                    });
                }
                RunStatus::Failed(error) => {
                    return Ok(AgentWaitResult {
                        status: WaitStatus::Error,
                        started_at_ms: started_at,
                        ended_at_ms: ended_at,
                        error: Some(error),
                    });
                }
                RunStatus::Accepted | RunStatus::Running => {}
            }

            if time::timeout(timeout, notify.notified()).await.is_err() {
                return Ok(AgentWaitResult {
                    status: WaitStatus::Timeout,
                    started_at_ms: started_at,
                    ended_at_ms: None,
                    error: None,
                });
            }
        }
    }

    pub async fn result(&self, run_id: &str) -> KelvinResult<Option<StoredRunResult>> {
        let record = self.get_record(run_id).await?;
        let guard = record.lock().await;
        match &guard.status {
            RunStatus::Completed(result) => Ok(Some(StoredRunResult::Completed(result.clone()))),
            RunStatus::Failed(error) => Ok(Some(StoredRunResult::Failed(error.clone()))),
            RunStatus::Accepted | RunStatus::Running => Ok(None),
        }
    }
}
