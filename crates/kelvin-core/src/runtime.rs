use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::time;

use crate::{
    now_ms, AgentRunRequest, AgentRunResult, AgentWaitResult, Brain, KelvinError, KelvinResult,
    WaitStatus,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Accepted,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunState {
    pub run_id: String,
    pub phase: RunPhase,
    pub started_at_ms: u128,
    pub ended_at_ms: Option<u128>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunAccepted {
    pub run_id: String,
    pub accepted_at_ms: u128,
}

#[derive(Debug, Clone)]
pub enum RunOutcome {
    Completed(AgentRunResult),
    Failed(String),
    Timeout,
}

#[derive(Debug, Clone)]
enum RunStatus {
    Accepted,
    Running,
    Completed(AgentRunResult),
    Failed(String),
}

#[derive(Debug)]
struct RunRecord {
    run_id: String,
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
                run_id: run_id.to_string(),
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

    pub async fn state(&self, run_id: &str) -> KelvinResult<RunState> {
        let record = self.get_record(run_id).await?;
        let guard = record.lock().await;
        let (phase, error) = match &guard.status {
            RunStatus::Accepted => (RunPhase::Accepted, None),
            RunStatus::Running => (RunPhase::Running, None),
            RunStatus::Completed(_) => (RunPhase::Completed, None),
            RunStatus::Failed(error) => (RunPhase::Failed, Some(error.clone())),
        };
        Ok(RunState {
            run_id: guard.run_id.clone(),
            phase,
            started_at_ms: guard.started_at_ms,
            ended_at_ms: guard.ended_at_ms,
            error,
        })
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

#[derive(Debug)]
pub struct LaneScheduler {
    use_global_lane: bool,
    global_lane: Arc<Mutex<()>>,
    session_lanes: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl Default for LaneScheduler {
    fn default() -> Self {
        Self::new(true)
    }
}

impl LaneScheduler {
    pub fn new(use_global_lane: bool) -> Self {
        Self {
            use_global_lane,
            global_lane: Arc::new(Mutex::new(())),
            session_lanes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn get_session_lane(&self, lane_key: &str) -> Arc<Mutex<()>> {
        let mut lanes = self.session_lanes.lock().await;
        lanes
            .entry(lane_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn run_in_lane<T, F>(&self, lane_key: &str, task: F) -> T
    where
        F: Future<Output = T>,
    {
        let session_lane = self.get_session_lane(lane_key).await;
        let _session_guard = session_lane.lock().await;

        if self.use_global_lane {
            let _global_guard = self.global_lane.lock().await;
            task.await
        } else {
            task.await
        }
    }
}

#[derive(Clone)]
pub struct CoreRuntime {
    brain: Arc<dyn Brain>,
    scheduler: Arc<LaneScheduler>,
    registry: Arc<RunRegistry>,
}

impl CoreRuntime {
    pub fn new(brain: Arc<dyn Brain>) -> Self {
        Self {
            brain,
            scheduler: Arc::new(LaneScheduler::new(false)),
            registry: Arc::new(RunRegistry::default()),
        }
    }

    pub fn with_components(
        brain: Arc<dyn Brain>,
        scheduler: Arc<LaneScheduler>,
        registry: Arc<RunRegistry>,
    ) -> Self {
        Self {
            brain,
            scheduler,
            registry,
        }
    }

    pub fn registry(&self) -> Arc<RunRegistry> {
        self.registry.clone()
    }

    pub async fn submit(&self, req: AgentRunRequest) -> KelvinResult<RunAccepted> {
        self.registry.register(&req.run_id).await?;

        let accepted = RunAccepted {
            run_id: req.run_id.clone(),
            accepted_at_ms: now_ms(),
        };

        let run_id = req.run_id.clone();
        let lane_key = req.session_key.clone();
        let brain = self.brain.clone();
        let scheduler = self.scheduler.clone();
        let registry = self.registry.clone();

        tokio::spawn(async move {
            if registry.mark_running(&run_id).await.is_err() {
                return;
            }

            let result = scheduler.run_in_lane(&lane_key, brain.run(req)).await;
            match result {
                Ok(run_result) => {
                    let _ = registry.mark_completed(&run_id, run_result).await;
                }
                Err(err) => {
                    let _ = registry.mark_failed(&run_id, err.to_string()).await;
                }
            }
        });

        Ok(accepted)
    }

    pub async fn state(&self, run_id: &str) -> KelvinResult<RunState> {
        self.registry.state(run_id).await
    }

    pub async fn wait(&self, run_id: &str, timeout_ms: u64) -> KelvinResult<AgentWaitResult> {
        self.registry.wait(run_id, timeout_ms).await
    }

    pub async fn wait_for_outcome(&self, run_id: &str, timeout_ms: u64) -> KelvinResult<RunOutcome> {
        let wait_result = self.wait(run_id, timeout_ms).await?;
        match wait_result.status {
            WaitStatus::Timeout => Ok(RunOutcome::Timeout),
            WaitStatus::Ok | WaitStatus::Error => {
                let result = self.registry.result(run_id).await?;
                match result {
                    Some(StoredRunResult::Completed(run)) => Ok(RunOutcome::Completed(run)),
                    Some(StoredRunResult::Failed(error)) => Ok(RunOutcome::Failed(error)),
                    None => Ok(RunOutcome::Timeout),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::Value;
    use tokio::sync::{Mutex, Notify};

    use crate::{
        AgentPayload, AgentRunMeta, AgentRunRequest, AgentRunResult, Brain, KelvinError,
        KelvinResult, WaitStatus,
    };

    use super::{CoreRuntime, LaneScheduler, RunOutcome, RunPhase, RunRegistry};

    #[derive(Clone)]
    struct RecordingBrain {
        order: Arc<Mutex<Vec<String>>>,
        delay_ms: u64,
    }

    #[async_trait]
    impl Brain for RecordingBrain {
        async fn run(&self, req: AgentRunRequest) -> KelvinResult<AgentRunResult> {
            self.order
                .lock()
                .await
                .push(format!("{}-start", req.run_id));
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            self.order.lock().await.push(format!("{}-end", req.run_id));
            Ok(AgentRunResult {
                payloads: vec![AgentPayload {
                    text: "ok".to_string(),
                    is_error: false,
                }],
                meta: AgentRunMeta {
                    duration_ms: self.delay_ms as u128,
                    provider: "test".to_string(),
                    model: "test".to_string(),
                    stop_reason: Some("completed".to_string()),
                    error: None,
                },
            })
        }
    }

    #[derive(Clone)]
    struct BlockingBrain {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl Brain for BlockingBrain {
        async fn run(&self, _req: AgentRunRequest) -> KelvinResult<AgentRunResult> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(AgentRunResult {
                payloads: vec![AgentPayload {
                    text: "released".to_string(),
                    is_error: false,
                }],
                meta: AgentRunMeta {
                    duration_ms: 0,
                    provider: "test".to_string(),
                    model: "test".to_string(),
                    stop_reason: Some("completed".to_string()),
                    error: None,
                },
            })
        }
    }

    #[derive(Clone)]
    struct FailingBrain;

    #[async_trait]
    impl Brain for FailingBrain {
        async fn run(&self, _req: AgentRunRequest) -> KelvinResult<AgentRunResult> {
            Err(KelvinError::Backend("forced failure".to_string()))
        }
    }

    fn build_request(run_id: &str, session: &str) -> AgentRunRequest {
        AgentRunRequest {
            run_id: run_id.to_string(),
            session_id: session.to_string(),
            session_key: session.to_string(),
            workspace_dir: ".".to_string(),
            prompt: "hello".to_string(),
            extra_system_prompt: None,
            timeout_ms: None,
            memory_query: None,
        }
    }

    fn assert_object_keys(value: &Value, keys: &[&str]) {
        let object = value.as_object().expect("serialized object");
        let mut found = object.keys().cloned().collect::<Vec<_>>();
        found.sort();
        let mut expected = keys.iter().map(|value| value.to_string()).collect::<Vec<_>>();
        expected.sort();
        assert_eq!(found, expected);
    }

    #[tokio::test]
    async fn runtime_transitions_running_and_completed() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = CoreRuntime::new(Arc::new(BlockingBrain {
            entered: entered.clone(),
            release: release.clone(),
        }));

        runtime
            .submit(build_request("run-phase", "session-phase"))
            .await
            .expect("submit");

        entered.notified().await;
        let running_state = runtime.state("run-phase").await.expect("running state");
        assert_eq!(running_state.phase, RunPhase::Running);

        release.notify_waiters();
        let wait = runtime.wait("run-phase", 2_000).await.expect("wait");
        assert_eq!(wait.status, WaitStatus::Ok);
        let completed_state = runtime.state("run-phase").await.expect("completed state");
        assert_eq!(completed_state.phase, RunPhase::Completed);
    }

    #[tokio::test]
    async fn runtime_wait_reports_timeout_for_active_run() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = CoreRuntime::new(Arc::new(BlockingBrain {
            entered: entered.clone(),
            release: release.clone(),
        }));

        runtime
            .submit(build_request("run-timeout", "session-timeout"))
            .await
            .expect("submit");
        entered.notified().await;

        let wait = runtime.wait("run-timeout", 20).await.expect("wait timeout");
        assert_eq!(wait.status, WaitStatus::Timeout);
        let state = runtime.state("run-timeout").await.expect("state");
        assert_eq!(state.phase, RunPhase::Running);

        release.notify_waiters();
    }

    #[tokio::test]
    async fn runtime_reports_failed_state_for_brain_errors() {
        let runtime = CoreRuntime::new(Arc::new(FailingBrain));
        runtime
            .submit(build_request("run-fail", "session-fail"))
            .await
            .expect("submit");

        let wait = runtime.wait("run-fail", 2_000).await.expect("wait");
        assert_eq!(wait.status, WaitStatus::Error);
        assert!(wait
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("forced failure"));
        let failed_state = runtime.state("run-fail").await.expect("state");
        assert_eq!(failed_state.phase, RunPhase::Failed);
    }

    #[tokio::test]
    async fn serializes_runs_in_same_session_lane() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let brain = Arc::new(RecordingBrain {
            order: order.clone(),
            delay_ms: 40,
        });
        let runtime = CoreRuntime::with_components(
            brain,
            Arc::new(LaneScheduler::new(false)),
            Arc::new(RunRegistry::default()),
        );

        runtime
            .submit(build_request("run-1", "session-a"))
            .await
            .expect("submit run-1");
        runtime
            .submit(build_request("run-2", "session-a"))
            .await
            .expect("submit run-2");

        let wait_one = runtime.wait("run-1", 2_000).await.expect("wait run-1");
        let wait_two = runtime.wait("run-2", 2_000).await.expect("wait run-2");

        assert_eq!(wait_one.status, WaitStatus::Ok);
        assert_eq!(wait_two.status, WaitStatus::Ok);

        let observed = order.lock().await.clone();
        assert_eq!(
            observed,
            vec![
                "run-1-start".to_string(),
                "run-1-end".to_string(),
                "run-2-start".to_string(),
                "run-2-end".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn wait_and_result_schema_is_stable() {
        let runtime = CoreRuntime::new(Arc::new(RecordingBrain {
            order: Arc::new(Mutex::new(Vec::new())),
            delay_ms: 1,
        }));
        runtime
            .submit(build_request("run-schema", "session-schema"))
            .await
            .expect("submit");

        let wait = runtime.wait("run-schema", 2_000).await.expect("wait");
        assert_eq!(wait.status, WaitStatus::Ok);
        let wait_json = serde_json::to_value(wait).expect("wait json");
        assert_object_keys(&wait_json, &["status", "started_at_ms", "ended_at_ms", "error"]);

        let outcome = runtime
            .wait_for_outcome("run-schema", 2_000)
            .await
            .expect("wait outcome");
        match outcome {
            RunOutcome::Completed(result) => {
                let result_json = serde_json::to_value(result).expect("result json");
                assert_object_keys(&result_json, &["payloads", "meta"]);
                let meta = &result_json["meta"];
                assert_object_keys(meta, &["duration_ms", "provider", "model", "stop_reason", "error"]);
            }
            _ => panic!("expected completed outcome"),
        }
    }

    #[tokio::test]
    async fn duplicate_run_id_returns_typed_error() {
        let runtime = CoreRuntime::new(Arc::new(RecordingBrain {
            order: Arc::new(Mutex::new(Vec::new())),
            delay_ms: 2,
        }));
        let request = build_request("duplicate-run", "session-dup");
        runtime.submit(request.clone()).await.expect("first submit");
        let error = runtime.submit(request).await.expect_err("duplicate run id");
        assert!(matches!(error, KelvinError::InvalidInput(_)));
    }
}
