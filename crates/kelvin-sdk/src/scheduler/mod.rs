mod cron;
mod persistence;
mod store;

use serde::{Deserialize, Serialize};

use kelvin_core::{now_ms, KelvinError, KelvinResult};

pub use store::{SchedulerStatus, SchedulerStore};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleReplyTarget {
    pub channel: String,
    pub account_id: String,
}

impl ScheduleReplyTarget {
    pub fn normalize(self) -> KelvinResult<Self> {
        let channel = self.channel.trim().to_ascii_lowercase();
        if !matches!(channel.as_str(), "telegram" | "slack" | "discord") {
            return Err(KelvinError::InvalidInput(format!(
                "reply_target.channel must be one of telegram, slack, discord (got '{}')",
                self.channel
            )));
        }
        let account_id = self.account_id.trim();
        if account_id.is_empty() {
            return Err(KelvinError::InvalidInput(
                "reply_target.account_id must not be empty".to_string(),
            ));
        }
        if account_id.chars().any(|ch| ch.is_control()) {
            return Err(KelvinError::InvalidInput(
                "reply_target.account_id must not contain control characters".to_string(),
            ));
        }
        Ok(Self {
            channel,
            account_id: account_id.to_string(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduledTask {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub session_id: Option<String>,
    pub workspace_dir: Option<String>,
    pub timeout_ms: Option<u64>,
    pub system_prompt: Option<String>,
    pub memory_query: Option<String>,
    pub reply_target: Option<ScheduleReplyTarget>,
    pub created_by_session: String,
    pub created_at_ms: u128,
    pub approval_reason: String,
    pub next_slot_at_ms: u128,
}

#[derive(Debug, Clone)]
pub struct NewScheduledTask {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub session_id: Option<String>,
    pub workspace_dir: Option<String>,
    pub timeout_ms: Option<u64>,
    pub system_prompt: Option<String>,
    pub memory_query: Option<String>,
    pub reply_target: Option<ScheduleReplyTarget>,
    pub created_by_session: String,
    pub created_at_ms: u128,
    pub approval_reason: String,
}

impl NewScheduledTask {
    pub fn into_task(self) -> KelvinResult<ScheduledTask> {
        validate_identifier("schedule id", &self.id, 128)?;
        let prompt = self.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(KelvinError::InvalidInput(
                "schedule prompt must not be empty".to_string(),
            ));
        }
        let approval_reason = self.approval_reason.trim().to_string();
        if approval_reason.is_empty() {
            return Err(KelvinError::InvalidInput(
                "schedule approval reason must not be empty".to_string(),
            ));
        }
        let created_by_session = self.created_by_session.trim().to_string();
        if created_by_session.is_empty() {
            return Err(KelvinError::InvalidInput(
                "schedule created_by_session must not be empty".to_string(),
            ));
        }

        let cron = cron::CronSchedule::parse(&self.cron)?;
        let created_at_ms = if self.created_at_ms == 0 {
            now_ms()
        } else {
            self.created_at_ms
        };

        Ok(ScheduledTask {
            id: self.id,
            cron: cron.raw().to_string(),
            prompt,
            session_id: trim_option(self.session_id),
            workspace_dir: trim_option(self.workspace_dir),
            timeout_ms: self.timeout_ms,
            system_prompt: trim_option(self.system_prompt),
            memory_query: trim_option(self.memory_query),
            reply_target: match self.reply_target {
                Some(target) => Some(target.normalize()?),
                None => None,
            },
            created_by_session,
            created_at_ms,
            approval_reason,
            next_slot_at_ms: cron.first_slot_at_or_after(minute_slot(created_at_ms))?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleSlotPhase {
    Claimed,
    SubmitFailed,
    Submitted,
    Completed,
    Failed,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleReplyDelivery {
    pub delivered: bool,
    pub attempted_at_ms: u128,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleSlotRecord {
    pub schedule_id: String,
    pub slot_at_ms: u128,
    pub claimed_at_ms: u128,
    pub phase: ScheduleSlotPhase,
    pub run_id: Option<String>,
    pub finished_at_ms: Option<u128>,
    pub error: Option<String>,
    pub response_preview: Option<String>,
    pub reply: Option<ScheduleReplyDelivery>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScheduleAuditEntry {
    pub ts_ms: u128,
    pub kind: String,
    pub schedule_id: Option<String>,
    pub slot_at_ms: Option<u128>,
    pub run_id: Option<String>,
    pub actor_session_id: Option<String>,
    pub message: String,
    #[serde(default)]
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimedScheduleSlot {
    pub schedule: ScheduledTask,
    pub slot_at_ms: u128,
}

pub(crate) const MINUTE_MS: u128 = 60_000;

pub(crate) fn minute_slot(ts_ms: u128) -> u128 {
    ts_ms / MINUTE_MS * MINUTE_MS
}

pub(crate) fn trim_option(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

pub(crate) fn truncate(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        value.to_string()
    } else {
        value.chars().take(max_len).collect::<String>()
    }
}

pub(crate) fn validate_identifier(label: &str, value: &str, max_len: usize) -> KelvinResult<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    if trimmed.len() > max_len {
        return Err(KelvinError::InvalidInput(format!(
            "{label} exceeds {max_len} bytes"
        )));
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not contain control characters"
        )));
    }
    Ok(())
}
