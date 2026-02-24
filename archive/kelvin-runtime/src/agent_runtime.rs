use std::sync::Arc;

use kelvin_core::{
    now_ms, AgentRunRequest, AgentRunResult, AgentWaitResult, Brain, KelvinResult, WaitStatus,
};

use crate::{LaneScheduler, RunRegistry, StoredRunResult};

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

#[derive(Clone)]
pub struct AgentRuntime {
    brain: Arc<dyn Brain>,
    scheduler: Arc<LaneScheduler>,
    registry: Arc<RunRegistry>,
}

impl AgentRuntime {
    pub fn new(
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
    use tokio::sync::Mutex;

    use kelvin_core::{
        AgentPayload, AgentRunMeta, AgentRunRequest, AgentRunResult, Brain, KelvinResult, WaitStatus,
    };

    use crate::{AgentRuntime, LaneScheduler, RunOutcome, RunRegistry};

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

    #[tokio::test]
    async fn serializes_runs_in_same_session_lane() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let brain = Arc::new(RecordingBrain {
            order: order.clone(),
            delay_ms: 40,
        });
        let runtime = AgentRuntime::new(
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
    async fn wait_reports_timeout_when_run_still_active() {
        let brain = Arc::new(RecordingBrain {
            order: Arc::new(Mutex::new(Vec::new())),
            delay_ms: 300,
        });
        let runtime = AgentRuntime::new(
            brain,
            Arc::new(LaneScheduler::new(false)),
            Arc::new(RunRegistry::default()),
        );

        runtime
            .submit(build_request("slow-run", "session-timeout"))
            .await
            .expect("submit slow run");

        let wait = runtime.wait("slow-run", 20).await.expect("wait timeout");
        assert_eq!(wait.status, WaitStatus::Timeout);
    }

    #[tokio::test]
    async fn wait_for_outcome_returns_completed_result() {
        let brain = Arc::new(RecordingBrain {
            order: Arc::new(Mutex::new(Vec::new())),
            delay_ms: 10,
        });
        let runtime = AgentRuntime::new(
            brain,
            Arc::new(LaneScheduler::new(false)),
            Arc::new(RunRegistry::default()),
        );

        runtime
            .submit(build_request("run-outcome", "session-outcome"))
            .await
            .expect("submit outcome run");

        let outcome = runtime
            .wait_for_outcome("run-outcome", 500)
            .await
            .expect("wait for outcome");

        match outcome {
            RunOutcome::Completed(result) => {
                assert_eq!(result.payloads.len(), 1);
            }
            _ => panic!("expected completed outcome"),
        }
    }
}
