use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use kelvin_core::{now_ms, KelvinError, KelvinResult};

use super::cron::CronSchedule;
use super::persistence::{load_state, migrate_legacy_if_needed, save_state, SchedulerState};
use super::{
    truncate, ClaimedScheduleSlot, NewScheduledTask, ScheduleAuditEntry, ScheduleReplyDelivery,
    ScheduleSlotPhase, ScheduleSlotRecord, ScheduledTask,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SchedulerStatus {
    pub schedule_count: usize,
    pub slot_count: usize,
    pub audit_count: usize,
    pub due_now_count: usize,
    pub next_slot_at_ms: Option<u128>,
}

#[derive(Debug, Clone)]
pub struct SchedulerStore {
    state_path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl SchedulerStore {
    pub fn new(state_dir: Option<PathBuf>, workspace_dir: &Path) -> KelvinResult<Self> {
        let root = state_dir.unwrap_or_else(|| workspace_dir.join(".kelvin").join("scheduler"));
        std::fs::create_dir_all(root.join("scheduler"))
            .map_err(|err| KelvinError::Io(format!("create scheduler dir: {err}")))?;
        let store = Self {
            state_path: root.join("scheduler").join("state.json"),
            lock: Arc::new(Mutex::new(())),
        };
        migrate_legacy_if_needed(&store.state_path, workspace_dir)?;
        Ok(store)
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    pub fn add_schedule(&self, task: NewScheduledTask) -> KelvinResult<ScheduledTask> {
        let task = task.into_task()?;
        let mut state = self.with_state_mut(|state| {
            if state.schedules.iter().any(|item| item.id == task.id) {
                return Err(KelvinError::InvalidInput(format!(
                    "schedule id already exists: {}",
                    task.id
                )));
            }
            state.schedules.push(task.clone());
            state.audit.push(ScheduleAuditEntry {
                ts_ms: now_ms(),
                kind: "schedule_added".to_string(),
                schedule_id: Some(task.id.clone()),
                slot_at_ms: None,
                run_id: None,
                actor_session_id: Some(task.created_by_session.clone()),
                message: format!("schedule '{}' added", task.id),
                detail: json!({
                    "cron": task.cron,
                    "next_slot_at_ms": task.next_slot_at_ms,
                    "has_reply_target": task.reply_target.is_some(),
                }),
            });
            Ok(())
        })?;
        save_state(&self.state_path, &mut state)?;
        Ok(task)
    }

    pub fn remove_schedule(
        &self,
        id: &str,
        actor_session_id: &str,
        approval_reason: &str,
    ) -> KelvinResult<bool> {
        let mut removed = false;
        let mut state = self.with_state_mut(|state| {
            let before = state.schedules.len();
            state.schedules.retain(|item| item.id != id);
            removed = state.schedules.len() != before;
            if removed {
                state.audit.push(ScheduleAuditEntry {
                    ts_ms: now_ms(),
                    kind: "schedule_removed".to_string(),
                    schedule_id: Some(id.to_string()),
                    slot_at_ms: None,
                    run_id: None,
                    actor_session_id: Some(actor_session_id.to_string()),
                    message: format!("schedule '{}' removed", id),
                    detail: json!({ "approval_reason": approval_reason }),
                });
            }
            Ok(())
        })?;
        if removed {
            save_state(&self.state_path, &mut state)?;
        }
        Ok(removed)
    }

    pub fn list_schedules(&self) -> KelvinResult<Vec<ScheduledTask>> {
        self.with_state(|state| state.schedules.clone())
    }

    pub fn recent_slots(
        &self,
        schedule_id: Option<&str>,
        limit: usize,
    ) -> KelvinResult<Vec<ScheduleSlotRecord>> {
        self.with_state(|state| {
            let mut slots = state.slots.clone();
            if let Some(schedule_id) = schedule_id {
                slots.retain(|slot| slot.schedule_id == schedule_id);
            }
            slots.sort_by(|left, right| right.slot_at_ms.cmp(&left.slot_at_ms));
            slots.truncate(limit.max(1));
            slots
        })
    }

    pub fn recent_audit(
        &self,
        schedule_id: Option<&str>,
        limit: usize,
    ) -> KelvinResult<Vec<ScheduleAuditEntry>> {
        self.with_state(|state| {
            let mut audit = state.audit.clone();
            if let Some(schedule_id) = schedule_id {
                audit.retain(|entry| entry.schedule_id.as_deref() == Some(schedule_id));
            }
            audit.sort_by(|left, right| right.ts_ms.cmp(&left.ts_ms));
            audit.truncate(limit.max(1));
            audit
        })
    }

    pub fn status(&self, now_ms: u128) -> KelvinResult<SchedulerStatus> {
        self.with_state(|state| SchedulerStatus {
            schedule_count: state.schedules.len(),
            slot_count: state.slots.len(),
            audit_count: state.audit.len(),
            due_now_count: state
                .schedules
                .iter()
                .filter(|task| task.next_slot_at_ms <= super::minute_slot(now_ms))
                .count(),
            next_slot_at_ms: state
                .schedules
                .iter()
                .map(|task| task.next_slot_at_ms)
                .min(),
        })
    }

    pub fn claim_due_slots(
        &self,
        now_ms: u128,
        max_per_schedule: usize,
    ) -> KelvinResult<Vec<ClaimedScheduleSlot>> {
        let due_cutoff = super::minute_slot(now_ms);
        let mut claimed = Vec::new();
        let mut state = self.with_state_mut(|state| {
            for schedule in &mut state.schedules {
                let cron = CronSchedule::parse(&schedule.cron)?;
                for _ in 0..max_per_schedule.max(1) {
                    if schedule.next_slot_at_ms > due_cutoff {
                        break;
                    }
                    let slot_at_ms = schedule.next_slot_at_ms;
                    if state.slots.iter().any(|slot| {
                        slot.schedule_id == schedule.id && slot.slot_at_ms == slot_at_ms
                    }) {
                        schedule.next_slot_at_ms = cron.next_slot_after(slot_at_ms)?;
                        continue;
                    }
                    state.slots.push(ScheduleSlotRecord {
                        schedule_id: schedule.id.clone(),
                        slot_at_ms,
                        claimed_at_ms: now_ms,
                        phase: ScheduleSlotPhase::Claimed,
                        run_id: None,
                        finished_at_ms: None,
                        error: None,
                        response_preview: None,
                        reply: None,
                    });
                    state.audit.push(ScheduleAuditEntry {
                        ts_ms: now_ms,
                        kind: "slot_claimed".to_string(),
                        schedule_id: Some(schedule.id.clone()),
                        slot_at_ms: Some(slot_at_ms),
                        run_id: None,
                        actor_session_id: None,
                        message: format!(
                            "claimed schedule slot '{}' at {}",
                            schedule.id, slot_at_ms
                        ),
                        detail: json!({}),
                    });
                    claimed.push(ClaimedScheduleSlot {
                        schedule: schedule.clone(),
                        slot_at_ms,
                    });
                    schedule.next_slot_at_ms = cron.next_slot_after(slot_at_ms)?;
                }
            }
            Ok(())
        })?;
        if !claimed.is_empty() {
            save_state(&self.state_path, &mut state)?;
        }
        Ok(claimed)
    }

    pub fn mark_slot_submitted(
        &self,
        schedule_id: &str,
        slot_at_ms: u128,
        run_id: &str,
    ) -> KelvinResult<()> {
        self.update_slot(schedule_id, slot_at_ms, |slot, audit| {
            slot.phase = ScheduleSlotPhase::Submitted;
            slot.run_id = Some(run_id.to_string());
            audit.push(ScheduleAuditEntry {
                ts_ms: now_ms(),
                kind: "slot_submitted".to_string(),
                schedule_id: Some(schedule_id.to_string()),
                slot_at_ms: Some(slot_at_ms),
                run_id: Some(run_id.to_string()),
                actor_session_id: None,
                message: format!("submitted run '{}' for schedule '{}'", run_id, schedule_id),
                detail: json!({}),
            });
        })
    }

    pub fn mark_slot_submit_failed(
        &self,
        schedule_id: &str,
        slot_at_ms: u128,
        error: &str,
    ) -> KelvinResult<()> {
        self.update_slot(schedule_id, slot_at_ms, |slot, audit| {
            slot.phase = ScheduleSlotPhase::SubmitFailed;
            slot.finished_at_ms = Some(now_ms());
            slot.error = Some(truncate(error, 512));
            audit.push(ScheduleAuditEntry {
                ts_ms: now_ms(),
                kind: "slot_submit_failed".to_string(),
                schedule_id: Some(schedule_id.to_string()),
                slot_at_ms: Some(slot_at_ms),
                run_id: None,
                actor_session_id: None,
                message: format!("schedule '{}' submit failed", schedule_id),
                detail: json!({ "error": truncate(error, 512) }),
            });
        })
    }

    pub fn mark_slot_outcome(
        &self,
        schedule_id: &str,
        slot_at_ms: u128,
        phase: ScheduleSlotPhase,
        run_id: &str,
        response_preview: Option<String>,
        error: Option<String>,
    ) -> KelvinResult<()> {
        self.update_slot(schedule_id, slot_at_ms, |slot, audit| {
            slot.phase = phase.clone();
            slot.run_id = Some(run_id.to_string());
            slot.finished_at_ms = Some(now_ms());
            slot.response_preview = response_preview.clone().map(|value| truncate(&value, 512));
            slot.error = error.clone().map(|value| truncate(&value, 512));
            audit.push(ScheduleAuditEntry {
                ts_ms: now_ms(),
                kind: format!("slot_{:?}", phase).to_ascii_lowercase(),
                schedule_id: Some(schedule_id.to_string()),
                slot_at_ms: Some(slot_at_ms),
                run_id: Some(run_id.to_string()),
                actor_session_id: None,
                message: format!("schedule '{}' finished with {:?}", schedule_id, phase),
                detail: json!({
                    "response_preview": slot.response_preview,
                    "error": slot.error,
                }),
            });
        })
    }

    pub fn mark_reply_result(
        &self,
        schedule_id: &str,
        slot_at_ms: u128,
        delivered: bool,
        error: Option<&str>,
    ) -> KelvinResult<()> {
        self.update_slot(schedule_id, slot_at_ms, |slot, audit| {
            slot.reply = Some(ScheduleReplyDelivery {
                delivered,
                attempted_at_ms: now_ms(),
                error: error.map(|value| truncate(value, 512)),
            });
            audit.push(ScheduleAuditEntry {
                ts_ms: now_ms(),
                kind: if delivered {
                    "reply_delivered".to_string()
                } else {
                    "reply_failed".to_string()
                },
                schedule_id: Some(schedule_id.to_string()),
                slot_at_ms: Some(slot_at_ms),
                run_id: slot.run_id.clone(),
                actor_session_id: None,
                message: if delivered {
                    format!("delivered reply for schedule '{}'", schedule_id)
                } else {
                    format!("reply delivery failed for schedule '{}'", schedule_id)
                },
                detail: json!({ "error": error.map(|value| truncate(value, 512)) }),
            });
        })
    }

    fn update_slot<F>(&self, schedule_id: &str, slot_at_ms: u128, mutate: F) -> KelvinResult<()>
    where
        F: FnOnce(&mut ScheduleSlotRecord, &mut Vec<ScheduleAuditEntry>),
    {
        let mut state = self.with_state_mut(|state| {
            let slot = state
                .slots
                .iter_mut()
                .find(|slot| slot.schedule_id == schedule_id && slot.slot_at_ms == slot_at_ms)
                .ok_or_else(|| {
                    KelvinError::NotFound(format!(
                        "unknown schedule slot: {}@{}",
                        schedule_id, slot_at_ms
                    ))
                })?;
            mutate(slot, &mut state.audit);
            Ok(())
        })?;
        save_state(&self.state_path, &mut state)
    }

    fn with_state<T>(&self, reader: impl FnOnce(&SchedulerState) -> T) -> KelvinResult<T> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = load_state(&self.state_path)?;
        Ok(reader(&state))
    }

    fn with_state_mut(
        &self,
        writer: impl FnOnce(&mut SchedulerState) -> KelvinResult<()>,
    ) -> KelvinResult<SchedulerState> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = load_state(&self.state_path)?;
        writer(&mut state)?;
        Ok(state)
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
