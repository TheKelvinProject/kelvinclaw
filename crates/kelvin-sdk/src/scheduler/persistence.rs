use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use kelvin_core::{now_ms, KelvinError, KelvinResult};

use super::{NewScheduledTask, ScheduleAuditEntry, ScheduleSlotRecord, ScheduledTask};

const MAX_AUDIT_ENTRIES: usize = 4_096;
const MAX_SLOT_ENTRIES: usize = 4_096;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(super) struct SchedulerState {
    #[serde(default)]
    pub(super) schedules: Vec<ScheduledTask>,
    #[serde(default)]
    pub(super) slots: Vec<ScheduleSlotRecord>,
    #[serde(default)]
    pub(super) audit: Vec<ScheduleAuditEntry>,
}

pub(super) fn load_state(path: &Path) -> KelvinResult<SchedulerState> {
    if !path.is_file() {
        return Ok(SchedulerState::default());
    }
    let bytes =
        fs::read(path).map_err(|err| KelvinError::Io(format!("read scheduler state: {err}")))?;
    match serde_json::from_slice::<SchedulerState>(&bytes) {
        Ok(state) => Ok(state),
        Err(err) => {
            quarantine_corrupt_file(path, &format!("invalid scheduler state json: {err}"));
            Ok(SchedulerState::default())
        }
    }
}

pub(super) fn save_state(path: &Path, state: &mut SchedulerState) -> KelvinResult<()> {
    state
        .schedules
        .sort_by(|left, right| left.id.cmp(&right.id));
    state.slots.sort_by(|left, right| {
        left.schedule_id
            .cmp(&right.schedule_id)
            .then_with(|| left.slot_at_ms.cmp(&right.slot_at_ms))
    });
    state
        .audit
        .sort_by(|left, right| left.ts_ms.cmp(&right.ts_ms));
    if state.slots.len() > MAX_SLOT_ENTRIES {
        let keep_from = state.slots.len().saturating_sub(MAX_SLOT_ENTRIES);
        state.slots.drain(..keep_from);
    }
    if state.audit.len() > MAX_AUDIT_ENTRIES {
        let keep_from = state.audit.len().saturating_sub(MAX_AUDIT_ENTRIES);
        state.audit.drain(..keep_from);
    }

    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|err| KelvinError::Io(format!("serialize scheduler state: {err}")))?;
    write_atomic(path, &bytes)
}

pub(super) fn migrate_legacy_if_needed(path: &Path, workspace_dir: &Path) -> KelvinResult<()> {
    let legacy_path = workspace_dir
        .join(".kelvin")
        .join("scheduler")
        .join("tasks.json");
    if path.is_file() || !legacy_path.is_file() {
        return Ok(());
    }

    let bytes = fs::read(&legacy_path)
        .map_err(|err| KelvinError::Io(format!("read legacy scheduler state: {err}")))?;
    let legacy: Vec<Value> = serde_json::from_slice(&bytes).map_err(|err| {
        KelvinError::InvalidInput(format!(
            "invalid legacy scheduler state '{}': {err}",
            legacy_path.to_string_lossy()
        ))
    })?;

    let mut state = SchedulerState::default();
    for item in legacy {
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let cron = item.get("cron").and_then(Value::as_str).unwrap_or_default();
        let prompt = item.get("task").and_then(Value::as_str).unwrap_or_default();
        let approval_reason = item
            .get("approval_reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if id.is_empty() || cron.is_empty() || prompt.is_empty() || approval_reason.is_empty() {
            continue;
        }

        let created_by_session = item
            .get("created_by_session")
            .and_then(Value::as_str)
            .unwrap_or("legacy");
        let task = NewScheduledTask {
            id: id.to_string(),
            cron: cron.to_string(),
            prompt: prompt.to_string(),
            session_id: None,
            workspace_dir: Some(workspace_dir.to_string_lossy().to_string()),
            timeout_ms: None,
            system_prompt: None,
            memory_query: None,
            reply_target: None,
            created_by_session: created_by_session.to_string(),
            created_at_ms: now_ms(),
            approval_reason: approval_reason.to_string(),
        }
        .into_task()?;

        state.schedules.push(task.clone());
        state.audit.push(ScheduleAuditEntry {
            ts_ms: now_ms(),
            kind: "legacy_migrated".to_string(),
            schedule_id: Some(task.id.clone()),
            slot_at_ms: None,
            run_id: None,
            actor_session_id: Some(task.created_by_session.clone()),
            message: format!("migrated legacy schedule '{}'", task.id),
            detail: serde_json::json!({}),
        });
    }

    save_state(path, &mut state)?;
    let renamed = legacy_path.with_extension(format!("json.migrated.{}", now_ms()));
    let _ = fs::rename(&legacy_path, renamed);
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> KelvinResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| KelvinError::Io(format!("create parent dir: {err}")))?;
    }
    let tmp_path = path.with_extension("tmp");
    let mut file = File::create(&tmp_path)
        .map_err(|err| KelvinError::Io(format!("create temp file: {err}")))?;
    file.write_all(bytes)
        .map_err(|err| KelvinError::Io(format!("write temp file: {err}")))?;
    file.sync_all()
        .map_err(|err| KelvinError::Io(format!("sync temp file: {err}")))?;
    fs::rename(&tmp_path, path).map_err(|err| KelvinError::Io(format!("replace file: {err}")))?;
    Ok(())
}

fn quarantine_corrupt_file(path: &Path, reason: &str) {
    let Some(file_name) = path.file_name().and_then(|item| item.to_str()) else {
        eprintln!(
            "warning: corrupt scheduler state could not be quarantined (invalid path '{}'): {}",
            path.to_string_lossy(),
            reason
        );
        return;
    };
    let renamed = path.with_file_name(format!("{file_name}.corrupt.{}", now_ms()));
    match fs::rename(path, &renamed) {
        Ok(()) => eprintln!(
            "warning: quarantined corrupt scheduler state '{}' as '{}' ({})",
            path.to_string_lossy(),
            renamed.to_string_lossy(),
            reason
        ),
        Err(err) => eprintln!(
            "warning: corrupt scheduler state '{}' detected but quarantine rename failed: {} ({})",
            path.to_string_lossy(),
            err,
            reason
        ),
    }
}
