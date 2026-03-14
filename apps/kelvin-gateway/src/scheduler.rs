use std::path::PathBuf;
use std::sync::Arc;

use kelvin_core::{now_ms, KelvinError, RunOutcome};
use kelvin_sdk::{
    ClaimedScheduleSlot, KelvinSdkRunRequest, KelvinSdkRuntime, ScheduleSlotPhase, SchedulerStore,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::channels::ChannelEngine;

const DEFAULT_TICK_MS: u64 = 1_000;
const DEFAULT_MAX_CLAIMS_PER_SCHEDULE: usize = 4;
const HISTORY_LIMIT_MAX: usize = 200;
const OUTCOME_PREVIEW_MAX_LEN: usize = 512;

#[derive(Debug, Default)]
struct SchedulerMetrics {
    last_scan_started_ms: Option<u128>,
    last_scan_finished_ms: Option<u128>,
    claimed_total: u64,
    submitted_total: u64,
    submit_failed_total: u64,
    completed_total: u64,
    failed_total: u64,
    timeout_total: u64,
    reply_delivered_total: u64,
    reply_failed_total: u64,
    last_error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct GatewayScheduler {
    store: Arc<SchedulerStore>,
    metrics: Arc<Mutex<SchedulerMetrics>>,
    tick_ms: u64,
    max_claims_per_schedule: usize,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ScheduleListParams {}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ScheduleHistoryParams {
    pub schedule_id: Option<String>,
    pub limit: Option<usize>,
}

impl GatewayScheduler {
    pub(crate) fn new(store: Arc<SchedulerStore>) -> Self {
        Self {
            store,
            metrics: Arc::new(Mutex::new(SchedulerMetrics::default())),
            tick_ms: DEFAULT_TICK_MS,
            max_claims_per_schedule: DEFAULT_MAX_CLAIMS_PER_SCHEDULE,
        }
    }

    pub(crate) fn start(&self, runtime: KelvinSdkRuntime, channels: Arc<Mutex<ChannelEngine>>) {
        let scheduler = self.clone();
        tokio::spawn(async move {
            loop {
                scheduler.scan_once(runtime.clone(), channels.clone()).await;
                sleep(Duration::from_millis(scheduler.tick_ms)).await;
            }
        });
    }

    pub(crate) async fn health_payload(&self) -> Value {
        let status = match self.store.status(now_ms()) {
            Ok(status) => serde_json::to_value(status).unwrap_or_else(|_| json!({})),
            Err(err) => {
                let message = format!("scheduler status unavailable: {err}");
                json!({
                    "schedule_count": 0,
                    "slot_count": 0,
                    "audit_count": 0,
                    "due_now_count": 0,
                    "next_slot_at_ms": null,
                    "error": message,
                })
            }
        };
        let metrics = self.metrics.lock().await;
        json!({
            "state_path": self.store.state_path().to_string_lossy(),
            "tick_ms": self.tick_ms,
            "max_claims_per_schedule": self.max_claims_per_schedule,
            "status": status,
            "metrics": {
                "last_scan_started_ms": metrics.last_scan_started_ms,
                "last_scan_finished_ms": metrics.last_scan_finished_ms,
                "claimed_total": metrics.claimed_total,
                "submitted_total": metrics.submitted_total,
                "submit_failed_total": metrics.submit_failed_total,
                "completed_total": metrics.completed_total,
                "failed_total": metrics.failed_total,
                "timeout_total": metrics.timeout_total,
                "reply_delivered_total": metrics.reply_delivered_total,
                "reply_failed_total": metrics.reply_failed_total,
                "last_error": metrics.last_error,
            }
        })
    }

    pub(crate) fn list_payload(&self) -> Result<Value, KelvinError> {
        Ok(json!({
            "state_path": self.store.state_path().to_string_lossy(),
            "status": self.store.status(now_ms())?,
            "schedules": self.store.list_schedules()?,
        }))
    }

    pub(crate) fn history_payload(
        &self,
        params: ScheduleHistoryParams,
    ) -> Result<Value, KelvinError> {
        let limit = params.limit.unwrap_or(20).clamp(1, HISTORY_LIMIT_MAX);
        let schedule_id = params.schedule_id.as_deref();
        Ok(json!({
            "state_path": self.store.state_path().to_string_lossy(),
            "schedule_id": params.schedule_id,
            "limit": limit,
            "slots": self.store.recent_slots(schedule_id, limit)?,
            "audit": self.store.recent_audit(schedule_id, limit)?,
        }))
    }

    async fn scan_once(&self, runtime: KelvinSdkRuntime, channels: Arc<Mutex<ChannelEngine>>) {
        {
            let mut metrics = self.metrics.lock().await;
            metrics.last_scan_started_ms = Some(now_ms());
        }

        let claimed = match self
            .store
            .claim_due_slots(now_ms(), self.max_claims_per_schedule)
        {
            Ok(claimed) => claimed,
            Err(err) => {
                self.record_error(format!("scheduler claim scan failed: {err}"))
                    .await;
                self.mark_scan_finished().await;
                return;
            }
        };

        if !claimed.is_empty() {
            let mut metrics = self.metrics.lock().await;
            metrics.claimed_total = metrics.claimed_total.saturating_add(claimed.len() as u64);
        }

        for slot in claimed {
            let scheduler = self.clone();
            let runtime = runtime.clone();
            let channels = channels.clone();
            tokio::spawn(async move {
                scheduler.execute_slot(runtime, channels, slot).await;
            });
        }

        self.mark_scan_finished().await;
    }

    async fn execute_slot(
        &self,
        runtime: KelvinSdkRuntime,
        channels: Arc<Mutex<ChannelEngine>>,
        claimed: ClaimedScheduleSlot,
    ) {
        let schedule = claimed.schedule;
        let run_id = format!("schedule-{}-{}", schedule.id, claimed.slot_at_ms);
        let request = KelvinSdkRunRequest {
            prompt: schedule.prompt.clone(),
            session_id: schedule.session_id.clone(),
            workspace_dir: schedule.workspace_dir.as_ref().map(PathBuf::from),
            timeout_ms: schedule.timeout_ms,
            system_prompt: schedule.system_prompt.clone(),
            memory_query: schedule.memory_query.clone(),
            run_id: Some(run_id.clone()),
        };

        let accepted = match runtime.submit(request).await {
            Ok(accepted) => accepted,
            Err(err) => {
                let message = err.to_string();
                let _ =
                    self.store
                        .mark_slot_submit_failed(&schedule.id, claimed.slot_at_ms, &message);
                let mut metrics = self.metrics.lock().await;
                metrics.submit_failed_total = metrics.submit_failed_total.saturating_add(1);
                metrics.last_error = Some(message);
                return;
            }
        };

        let _ = self
            .store
            .mark_slot_submitted(&schedule.id, claimed.slot_at_ms, &accepted.run_id);
        {
            let mut metrics = self.metrics.lock().await;
            metrics.submitted_total = metrics.submitted_total.saturating_add(1);
        }

        let timeout_ms = schedule.timeout_ms.unwrap_or(30_000).saturating_add(3_000);
        match runtime.wait_for_outcome(&accepted.run_id, timeout_ms).await {
            Ok(RunOutcome::Completed(result)) => {
                let preview = join_payloads(
                    &result
                        .payloads
                        .iter()
                        .map(|item| item.text.clone())
                        .collect::<Vec<_>>(),
                );
                let _ = self.store.mark_slot_outcome(
                    &schedule.id,
                    claimed.slot_at_ms,
                    ScheduleSlotPhase::Completed,
                    &accepted.run_id,
                    Some(preview.clone()),
                    None,
                );
                {
                    let mut metrics = self.metrics.lock().await;
                    metrics.completed_total = metrics.completed_total.saturating_add(1);
                }
                self.deliver_reply(channels, &schedule, claimed.slot_at_ms, &preview)
                    .await;
            }
            Ok(RunOutcome::Failed(error)) => {
                let text = truncate_text(&format!("Kelvin scheduled run failed: {error}"));
                let _ = self.store.mark_slot_outcome(
                    &schedule.id,
                    claimed.slot_at_ms,
                    ScheduleSlotPhase::Failed,
                    &accepted.run_id,
                    None,
                    Some(error.clone()),
                );
                let mut metrics = self.metrics.lock().await;
                metrics.failed_total = metrics.failed_total.saturating_add(1);
                metrics.last_error = Some(error);
                drop(metrics);
                self.deliver_reply(channels, &schedule, claimed.slot_at_ms, &text)
                    .await;
            }
            Ok(RunOutcome::Timeout) => {
                let _ = self.store.mark_slot_outcome(
                    &schedule.id,
                    claimed.slot_at_ms,
                    ScheduleSlotPhase::Timeout,
                    &accepted.run_id,
                    None,
                    None,
                );
                let mut metrics = self.metrics.lock().await;
                metrics.timeout_total = metrics.timeout_total.saturating_add(1);
                metrics.last_error = Some("scheduled run timed out".to_string());
                drop(metrics);
                self.deliver_reply(
                    channels,
                    &schedule,
                    claimed.slot_at_ms,
                    "Kelvin scheduled run timed out.",
                )
                .await;
            }
            Err(err) => {
                let message = err.to_string();
                let _ =
                    self.store
                        .mark_slot_submit_failed(&schedule.id, claimed.slot_at_ms, &message);
                let mut metrics = self.metrics.lock().await;
                metrics.submit_failed_total = metrics.submit_failed_total.saturating_add(1);
                metrics.last_error = Some(message);
            }
        }
    }

    async fn deliver_reply(
        &self,
        channels: Arc<Mutex<ChannelEngine>>,
        schedule: &kelvin_sdk::ScheduledTask,
        slot_at_ms: u128,
        text: &str,
    ) {
        let Some(target) = schedule.reply_target.as_ref() else {
            return;
        };

        let result = channels
            .lock()
            .await
            .deliver_scheduled_reply(target, text)
            .await;

        let mut metrics = self.metrics.lock().await;
        match result {
            Ok(()) => {
                let _ = self
                    .store
                    .mark_reply_result(&schedule.id, slot_at_ms, true, None);
                metrics.reply_delivered_total = metrics.reply_delivered_total.saturating_add(1);
            }
            Err(err) => {
                let error = err.to_string();
                let _ = self
                    .store
                    .mark_reply_result(&schedule.id, slot_at_ms, false, Some(&error));
                metrics.reply_failed_total = metrics.reply_failed_total.saturating_add(1);
                metrics.last_error = Some(error);
            }
        }
    }

    async fn mark_scan_finished(&self) {
        self.metrics.lock().await.last_scan_finished_ms = Some(now_ms());
    }

    async fn record_error(&self, error: String) {
        self.metrics.lock().await.last_error = Some(error);
    }
}

fn join_payloads(payloads: &[String]) -> String {
    let joined = payloads
        .iter()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    if joined.trim().is_empty() {
        "No response generated.".to_string()
    } else {
        truncate_text(&joined)
    }
}

fn truncate_text(value: &str) -> String {
    if value.chars().count() <= OUTCOME_PREVIEW_MAX_LEN {
        value.to_string()
    } else {
        value
            .chars()
            .take(OUTCOME_PREVIEW_MAX_LEN)
            .collect::<String>()
    }
}
