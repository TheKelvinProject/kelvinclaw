use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::redirect::Policy as RedirectPolicy;
use reqwest::Client;
use serde_json::{json, Map, Value};
use url::Url;

use kelvin_core::{
    now_ms, InMemoryPluginRegistry, KelvinError, KelvinResult, PluginCapability, PluginFactory,
    PluginManifest, PluginRegistry, PluginSecurityPolicy, SdkToolRegistry, Tool, ToolCallInput,
    ToolCallResult, ToolRegistry, KELVIN_CORE_API_VERSION,
};

const DEFAULT_READ_MAX_BYTES: usize = 64 * 1024;
const DEFAULT_FETCH_MAX_BYTES: usize = 128 * 1024;
const DEFAULT_FETCH_TIMEOUT_MS: u64 = 3_000;
const DEFAULT_WEB_ALLOW_HOSTS: &str = "docs.rs,crates.io,raw.githubusercontent.com,api.openai.com";

const ENV_TOOLPACK_ENABLE_FS_WRITE: &str = "KELVIN_TOOLPACK_ENABLE_FS_WRITE";
const ENV_TOOLPACK_ENABLE_WEB_FETCH: &str = "KELVIN_TOOLPACK_ENABLE_WEB_FETCH";
const ENV_TOOLPACK_ENABLE_SCHEDULER_WRITE: &str = "KELVIN_TOOLPACK_ENABLE_SCHEDULER_WRITE";
const ENV_TOOLPACK_ENABLE_SESSION_CLEAR: &str = "KELVIN_TOOLPACK_ENABLE_SESSION_CLEAR";
const ENV_TOOLPACK_WEB_ALLOW_HOSTS: &str = "KELVIN_TOOLPACK_WEB_ALLOW_HOSTS";

#[derive(Clone)]
struct ToolPackPolicy {
    allow_fs_write: bool,
    allow_web_fetch: bool,
    allow_scheduler_write: bool,
    allow_session_clear: bool,
    max_read_bytes: usize,
    max_fetch_bytes: usize,
    web_allow_hosts: Vec<String>,
}

impl ToolPackPolicy {
    fn from_env() -> Self {
        Self {
            allow_fs_write: env_bool(ENV_TOOLPACK_ENABLE_FS_WRITE, true),
            allow_web_fetch: env_bool(ENV_TOOLPACK_ENABLE_WEB_FETCH, true),
            allow_scheduler_write: env_bool(ENV_TOOLPACK_ENABLE_SCHEDULER_WRITE, true),
            allow_session_clear: env_bool(ENV_TOOLPACK_ENABLE_SESSION_CLEAR, true),
            max_read_bytes: DEFAULT_READ_MAX_BYTES,
            max_fetch_bytes: DEFAULT_FETCH_MAX_BYTES,
            web_allow_hosts: parse_host_allowlist(
                &std::env::var(ENV_TOOLPACK_WEB_ALLOW_HOSTS)
                    .unwrap_or_else(|_| DEFAULT_WEB_ALLOW_HOSTS.to_string()),
            ),
        }
    }
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => default,
    }
}

fn parse_host_allowlist(raw: &str) -> Vec<String> {
    let mut out = raw
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.to_ascii_lowercase())
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    out
}

fn args_object<'a>(
    args: &'a Value,
    tool_name: &str,
) -> KelvinResult<&'a serde_json::Map<String, Value>> {
    args.as_object().ok_or_else(|| {
        KelvinError::InvalidInput(format!("{tool_name} expects JSON object arguments"))
    })
}

fn required_string(
    args: &serde_json::Map<String, Value>,
    field: &str,
    tool_name: &str,
) -> KelvinResult<String> {
    let value = args.get(field).and_then(Value::as_str).ok_or_else(|| {
        KelvinError::InvalidInput(format!("{tool_name} requires string argument '{field}'"))
    })?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{tool_name} argument '{field}' must not be empty"
        )));
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        return Err(KelvinError::InvalidInput(format!(
            "{tool_name} argument '{field}' must not contain control characters"
        )));
    }
    Ok(trimmed.to_string())
}

fn optional_u64(
    args: &serde_json::Map<String, Value>,
    field: &str,
    tool_name: &str,
) -> KelvinResult<Option<u64>> {
    match args.get(field) {
        None => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(|| {
            KelvinError::InvalidInput(format!("{tool_name} argument '{field}' must be a u64"))
        }),
    }
}

fn optional_string(
    args: &serde_json::Map<String, Value>,
    field: &str,
    tool_name: &str,
) -> KelvinResult<Option<String>> {
    match args.get(field) {
        None => Ok(None),
        Some(value) => value.as_str().map(|v| Some(v.to_string())).ok_or_else(|| {
            KelvinError::InvalidInput(format!("{tool_name} argument '{field}' must be a string"))
        }),
    }
}

fn normalize_workspace_relative_path(path: &str, field_name: &str) -> KelvinResult<String> {
    let normalized = path.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{field_name} must not be empty"
        )));
    }
    if Path::new(&normalized).is_absolute() || normalized.starts_with('/') {
        return Err(KelvinError::InvalidInput(format!(
            "{field_name} must be a relative path"
        )));
    }
    if Path::new(&normalized)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(KelvinError::InvalidInput(format!(
            "{field_name} path traversal is not allowed"
        )));
    }
    Ok(normalized)
}

fn deny_sensitive_read_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || lower.starts_with(".git/")
        || lower.starts_with(".kelvin/plugins/")
}

fn require_sensitive_approval(
    args: &serde_json::Map<String, Value>,
    capability: &str,
) -> KelvinResult<String> {
    let Some(approval) = args.get("approval").and_then(Value::as_object) else {
        return Err(KelvinError::InvalidInput(format!(
            "sensitive operation '{capability}' denied by default; provide approval={{\"granted\":true,\"reason\":\"...\"}}"
        )));
    };
    let granted = approval
        .get("granted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !granted {
        return Err(KelvinError::InvalidInput(format!(
            "sensitive operation '{capability}' requires approval.granted=true"
        )));
    }
    let reason = approval
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if reason.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "sensitive operation '{capability}' requires non-empty approval.reason"
        )));
    }
    if reason.chars().count() > 256 || reason.chars().any(|ch| ch.is_control()) {
        return Err(KelvinError::InvalidInput(format!(
            "approval.reason for '{capability}' is invalid"
        )));
    }
    Ok(reason.to_string())
}

fn host_allowed(host: &str, allowlist: &[String]) -> bool {
    let candidate = host.trim().to_ascii_lowercase();
    if candidate.is_empty() {
        return false;
    }
    allowlist.iter().any(|pattern| {
        if let Some(suffix) = pattern.strip_prefix("*.") {
            candidate == suffix || candidate.ends_with(&format!(".{suffix}"))
        } else {
            candidate == *pattern
        }
    })
}

fn clamp_usize(raw: u64, max_allowed: usize) -> usize {
    match usize::try_from(raw) {
        Ok(value) => value.min(max_allowed),
        Err(_) => max_allowed,
    }
}

#[derive(Clone)]
struct SafeFsReadTool {
    policy: ToolPackPolicy,
}

#[async_trait]
impl Tool for SafeFsReadTool {
    fn name(&self) -> &str {
        "fs_safe_read"
    }

    async fn call(&self, input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        let args = args_object(&input.arguments, self.name())?;
        let path = normalize_workspace_relative_path(
            &required_string(args, "path", self.name())?,
            "path",
        )?;
        if deny_sensitive_read_path(&path) {
            return Err(KelvinError::InvalidInput(format!(
                "{} denied path '{}' by policy",
                self.name(),
                path
            )));
        }

        let requested_limit = optional_u64(args, "max_bytes", self.name())?
            .map(|value| clamp_usize(value, self.policy.max_read_bytes))
            .unwrap_or(self.policy.max_read_bytes);
        let read_limit = requested_limit.max(1);

        let abs = Path::new(&input.workspace_dir).join(&path);
        if !abs.is_file() {
            return Err(KelvinError::NotFound(format!(
                "{} path not found: {}",
                self.name(),
                path
            )));
        }

        let mut file = File::open(&abs)?;
        let mut buffer = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take((read_limit as u64).saturating_add(1))
            .read_to_end(&mut buffer)?;
        let truncated = buffer.len() > read_limit;
        if truncated {
            buffer.truncate(read_limit);
        }

        let content = String::from_utf8_lossy(&buffer).to_string();
        let output = json!({
            "path": path,
            "bytes": buffer.len(),
            "truncated": truncated,
            "content": content,
        });
        let summary = format!(
            "{} read '{}' ({} bytes{})",
            self.name(),
            path,
            buffer.len(),
            if truncated { ", truncated" } else { "" }
        );
        Ok(ToolCallResult {
            summary: summary.clone(),
            output: Some(output.to_string()),
            visible_text: Some(summary),
            is_error: false,
        })
    }
}

#[derive(Clone)]
struct SafeFsWriteTool {
    policy: ToolPackPolicy,
}

impl SafeFsWriteTool {
    fn write_scope_allowed(path: &str) -> bool {
        path.starts_with(".kelvin/sandbox/")
            || path.starts_with("memory/")
            || path.starts_with("notes/")
    }
}

#[async_trait]
impl Tool for SafeFsWriteTool {
    fn name(&self) -> &str {
        "fs_safe_write"
    }

    async fn call(&self, input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        if !self.policy.allow_fs_write {
            return Err(KelvinError::InvalidInput(format!(
                "{} is disabled by runtime policy; set {}=1 to enable",
                self.name(),
                ENV_TOOLPACK_ENABLE_FS_WRITE
            )));
        }
        let args = args_object(&input.arguments, self.name())?;
        let approval_reason = require_sensitive_approval(args, "filesystem_write")?;
        let path = normalize_workspace_relative_path(
            &required_string(args, "path", self.name())?,
            "path",
        )?;
        if !Self::write_scope_allowed(&path) {
            return Err(KelvinError::InvalidInput(format!(
                "{} denied path '{}'; allowed roots are .kelvin/sandbox/, memory/, notes/",
                self.name(),
                path
            )));
        }
        let content = required_string(args, "content", self.name())?;
        let mode = optional_string(args, "mode", self.name())?
            .unwrap_or_else(|| "overwrite".to_string())
            .to_ascii_lowercase();
        if mode != "overwrite" && mode != "append" {
            return Err(KelvinError::InvalidInput(format!(
                "{} argument 'mode' must be 'overwrite' or 'append'",
                self.name()
            )));
        }

        let abs = Path::new(&input.workspace_dir).join(&path);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut writer = OpenOptions::new()
            .create(true)
            .write(true)
            .append(mode == "append")
            .truncate(mode == "overwrite")
            .open(&abs)?;
        writer.write_all(content.as_bytes())?;
        writer.flush()?;

        let output = json!({
            "path": path,
            "mode": mode,
            "bytes_written": content.len(),
            "approval_reason": approval_reason,
        });
        let summary = format!(
            "{} wrote {} bytes to '{}' ({})",
            self.name(),
            content.len(),
            path,
            mode
        );
        Ok(ToolCallResult {
            summary: summary.clone(),
            output: Some(output.to_string()),
            visible_text: Some(summary),
            is_error: false,
        })
    }
}

#[derive(Clone)]
struct SafeWebFetchTool {
    policy: ToolPackPolicy,
    client: Client,
}

impl SafeWebFetchTool {
    fn try_new(policy: ToolPackPolicy) -> KelvinResult<Self> {
        let client = Client::builder()
            .redirect(RedirectPolicy::none())
            .timeout(Duration::from_millis(DEFAULT_FETCH_TIMEOUT_MS))
            .build()
            .map_err(|err| KelvinError::Backend(format!("build web fetch client: {err}")))?;
        Ok(Self { policy, client })
    }
}

#[async_trait]
impl Tool for SafeWebFetchTool {
    fn name(&self) -> &str {
        "web_fetch_safe"
    }

    async fn call(&self, input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        if !self.policy.allow_web_fetch {
            return Err(KelvinError::InvalidInput(format!(
                "{} is disabled by runtime policy; set {}=1 to enable",
                self.name(),
                ENV_TOOLPACK_ENABLE_WEB_FETCH
            )));
        }
        let args = args_object(&input.arguments, self.name())?;
        let approval_reason = require_sensitive_approval(args, "web_fetch")?;
        let url_raw = required_string(args, "url", self.name())?;
        let timeout_ms = optional_u64(args, "timeout_ms", self.name())?
            .unwrap_or(DEFAULT_FETCH_TIMEOUT_MS)
            .clamp(100, 30_000);
        let max_bytes = optional_u64(args, "max_bytes", self.name())?
            .map(|value| clamp_usize(value, self.policy.max_fetch_bytes))
            .unwrap_or(self.policy.max_fetch_bytes)
            .max(1);

        let parsed = Url::parse(&url_raw).map_err(|err| {
            KelvinError::InvalidInput(format!("{} invalid url '{}': {err}", self.name(), url_raw))
        })?;
        let scheme = parsed.scheme().to_ascii_lowercase();
        if scheme != "https" && scheme != "http" {
            return Err(KelvinError::InvalidInput(format!(
                "{} only supports http/https urls",
                self.name()
            )));
        }
        let host = parsed.host_str().ok_or_else(|| {
            KelvinError::InvalidInput(format!("{} url host is required", self.name()))
        })?;
        if !host_allowed(host, &self.policy.web_allow_hosts) {
            return Err(KelvinError::InvalidInput(format!(
                "{} denied host '{}'; allowed hosts: {}",
                self.name(),
                host,
                self.policy.web_allow_hosts.join(",")
            )));
        }

        let response = self
            .client
            .get(parsed.clone())
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await
            .map_err(|err| {
                KelvinError::Backend(format!("{} request failed: {err}", self.name()))
            })?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body_bytes = response.bytes().await.map_err(|err| {
            KelvinError::Backend(format!("{} read body failed: {err}", self.name()))
        })?;
        if body_bytes.len() > max_bytes {
            return Err(KelvinError::InvalidInput(format!(
                "{} response size {} exceeds limit {}",
                self.name(),
                body_bytes.len(),
                max_bytes
            )));
        }
        let body_text = String::from_utf8_lossy(&body_bytes).to_string();

        let output = json!({
            "url": parsed.as_str(),
            "host": host,
            "status": status,
            "content_type": content_type,
            "bytes": body_bytes.len(),
            "body": body_text,
            "approval_reason": approval_reason,
        });
        let summary = format!(
            "{} fetched {} (status={}, bytes={})",
            self.name(),
            parsed.as_str(),
            status,
            body_bytes.len()
        );
        Ok(ToolCallResult {
            summary: summary.clone(),
            output: Some(output.to_string()),
            visible_text: Some(summary),
            is_error: status >= 400,
        })
    }
}

#[derive(Clone)]
struct SchedulerTool {
    policy: ToolPackPolicy,
}

impl SchedulerTool {
    fn file_path(workspace: &str) -> std::path::PathBuf {
        Path::new(workspace).join(".kelvin/scheduler/tasks.json")
    }

    fn validate_cron(raw: &str) -> KelvinResult<String> {
        let value = raw.trim();
        let parts = value.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 5 {
            return Err(KelvinError::InvalidInput(
                "cron must have exactly 5 fields".to_string(),
            ));
        }
        if parts.iter().any(|part| {
            part.is_empty()
                || part.chars().count() > 32
                || !part
                    .chars()
                    .all(|ch| ch.is_ascii_digit() || matches!(ch, '*' | ',' | '-' | '/'))
        }) {
            return Err(KelvinError::InvalidInput(
                "cron contains unsupported characters".to_string(),
            ));
        }
        Ok(value.to_string())
    }
}

#[async_trait]
impl Tool for SchedulerTool {
    fn name(&self) -> &str {
        "schedule_cron"
    }

    async fn call(&self, input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        let args = args_object(&input.arguments, self.name())?;
        let action = optional_string(args, "action", self.name())?
            .unwrap_or_else(|| "list".to_string())
            .to_ascii_lowercase();
        let file_path = Self::file_path(&input.workspace_dir);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut tasks = if file_path.is_file() {
            let bytes = fs::read(&file_path)?;
            serde_json::from_slice::<Vec<Value>>(&bytes).map_err(|err| {
                KelvinError::InvalidInput(format!("{} invalid scheduler state: {err}", self.name()))
            })?
        } else {
            Vec::new()
        };

        match action.as_str() {
            "list" => {}
            "add" => {
                if !self.policy.allow_scheduler_write {
                    return Err(KelvinError::InvalidInput(format!(
                        "{} add is disabled by runtime policy; set {}=1",
                        self.name(),
                        ENV_TOOLPACK_ENABLE_SCHEDULER_WRITE
                    )));
                }
                let approval_reason = require_sensitive_approval(args, "schedule_mutation")?;
                let cron = Self::validate_cron(&required_string(args, "cron", self.name())?)?;
                let task = required_string(args, "task", self.name())?;
                let id = optional_string(args, "id", self.name())?
                    .unwrap_or_else(|| format!("task-{}", now_ms()));
                tasks.push(json!({
                    "id": id,
                    "cron": cron,
                    "task": task,
                    "created_by_session": input.session_id,
                    "approval_reason": approval_reason,
                }));
            }
            "remove" => {
                if !self.policy.allow_scheduler_write {
                    return Err(KelvinError::InvalidInput(format!(
                        "{} remove is disabled by runtime policy; set {}=1",
                        self.name(),
                        ENV_TOOLPACK_ENABLE_SCHEDULER_WRITE
                    )));
                }
                let _approval_reason = require_sensitive_approval(args, "schedule_mutation")?;
                let id = required_string(args, "id", self.name())?;
                tasks.retain(|item| item.get("id").and_then(Value::as_str) != Some(id.as_str()));
            }
            _ => {
                return Err(KelvinError::InvalidInput(format!(
                    "{} action must be one of: list, add, remove",
                    self.name()
                )));
            }
        }

        tasks.sort_by(|left, right| {
            let left_id = left.get("id").and_then(Value::as_str).unwrap_or_default();
            let right_id = right.get("id").and_then(Value::as_str).unwrap_or_default();
            left_id.cmp(right_id)
        });
        fs::write(
            &file_path,
            serde_json::to_vec_pretty(&tasks).unwrap_or_default(),
        )?;

        let summary = format!("{} action='{}' tasks={}", self.name(), action, tasks.len());
        let output = json!({
            "action": action,
            "count": tasks.len(),
            "tasks": tasks,
            "state_path": file_path.to_string_lossy(),
        });
        Ok(ToolCallResult {
            summary: summary.clone(),
            output: Some(output.to_string()),
            visible_text: Some(summary),
            is_error: false,
        })
    }
}

#[derive(Clone)]
struct SessionToolsTool {
    policy: ToolPackPolicy,
}

impl SessionToolsTool {
    fn state_path(workspace: &str, session_id: &str) -> std::path::PathBuf {
        Path::new(workspace)
            .join(".kelvin/session-tools")
            .join(format!("{session_id}.json"))
    }
}

#[async_trait]
impl Tool for SessionToolsTool {
    fn name(&self) -> &str {
        "session_tools"
    }

    async fn call(&self, input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        let args = args_object(&input.arguments, self.name())?;
        let action = optional_string(args, "action", self.name())?
            .unwrap_or_else(|| "list_notes".to_string())
            .to_ascii_lowercase();

        let path = Self::state_path(&input.workspace_dir, &input.session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut state = if path.is_file() {
            let bytes = fs::read(&path)?;
            serde_json::from_slice::<Map<String, Value>>(&bytes).map_err(|err| {
                KelvinError::InvalidInput(format!("{} invalid session state: {err}", self.name()))
            })?
        } else {
            Map::new()
        };
        if !state.contains_key("notes") {
            state.insert("notes".to_string(), json!([]));
        }

        match action.as_str() {
            "list_notes" => {}
            "append_note" => {
                let note = required_string(args, "note", self.name())?;
                let notes = state
                    .get_mut("notes")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| {
                        KelvinError::InvalidInput("session notes state is malformed".to_string())
                    })?;
                notes.push(json!({
                    "text": note,
                    "run_id": input.run_id,
                    "ts_ms": now_ms(),
                }));
            }
            "clear_notes" => {
                if !self.policy.allow_session_clear {
                    return Err(KelvinError::InvalidInput(format!(
                        "{} clear is disabled by runtime policy; set {}=1",
                        self.name(),
                        ENV_TOOLPACK_ENABLE_SESSION_CLEAR
                    )));
                }
                let _approval_reason = require_sensitive_approval(args, "session_clear")?;
                state.insert("notes".to_string(), json!([]));
            }
            _ => {
                return Err(KelvinError::InvalidInput(format!(
                    "{} action must be one of: list_notes, append_note, clear_notes",
                    self.name()
                )));
            }
        }

        fs::write(&path, serde_json::to_vec_pretty(&state).unwrap_or_default())?;
        let note_count = state
            .get("notes")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0);
        let summary = format!("{} action='{}' notes={}", self.name(), action, note_count);
        let output = json!({
            "action": action,
            "session_id": input.session_id,
            "state_path": path.to_string_lossy(),
            "notes": state.get("notes").cloned().unwrap_or_else(|| json!([])),
        });
        Ok(ToolCallResult {
            summary: summary.clone(),
            output: Some(output.to_string()),
            visible_text: Some(summary),
            is_error: false,
        })
    }
}

#[derive(Clone)]
struct SingleToolPlugin {
    manifest: PluginManifest,
    tool: Arc<dyn Tool>,
}

impl PluginFactory for SingleToolPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn tool(&self) -> Option<Arc<dyn Tool>> {
        Some(self.tool.clone())
    }
}

fn manifest(
    id: &str,
    name: &str,
    capabilities: Vec<PluginCapability>,
    description: &str,
) -> PluginManifest {
    PluginManifest {
        id: id.to_string(),
        name: name.to_string(),
        version: "0.1.0".to_string(),
        api_version: KELVIN_CORE_API_VERSION.to_string(),
        description: Some(description.to_string()),
        homepage: Some("https://github.com/agentichighway/kelvinclaw".to_string()),
        capabilities,
        experimental: false,
        min_core_version: Some("0.1.0".to_string()),
        max_core_version: None,
    }
}

pub fn load_default_toolpack_plugins(
    core_version: &str,
) -> KelvinResult<(Arc<dyn ToolRegistry>, usize)> {
    let policy = ToolPackPolicy::from_env();
    let registry = InMemoryPluginRegistry::new();
    let registration_policy = PluginSecurityPolicy {
        allow_fs_read: true,
        allow_fs_write: true,
        allow_network_egress: true,
        ..PluginSecurityPolicy::default()
    };

    let plugins = vec![
        SingleToolPlugin {
            manifest: manifest(
                "kelvin.tool.fs_read",
                "Kelvin Safe FS Read Tool",
                vec![PluginCapability::ToolProvider, PluginCapability::FsRead],
                "Workspace-scoped filesystem read with explicit path safety checks.",
            ),
            tool: Arc::new(SafeFsReadTool {
                policy: policy.clone(),
            }),
        },
        SingleToolPlugin {
            manifest: manifest(
                "kelvin.tool.fs_write",
                "Kelvin Safe FS Write Tool",
                vec![PluginCapability::ToolProvider, PluginCapability::FsWrite],
                "Workspace-scoped filesystem write with explicit approval and deny-by-default path policy.",
            ),
            tool: Arc::new(SafeFsWriteTool {
                policy: policy.clone(),
            }),
        },
        SingleToolPlugin {
            manifest: manifest(
                "kelvin.tool.web_fetch",
                "Kelvin Safe Web Fetch Tool",
                vec![
                    PluginCapability::ToolProvider,
                    PluginCapability::NetworkEgress,
                ],
                "Host-mediated web fetch with strict host allowlist and payload bounds.",
            ),
            tool: Arc::new(SafeWebFetchTool::try_new(policy.clone())?),
        },
        SingleToolPlugin {
            manifest: manifest(
                "kelvin.tool.scheduler",
                "Kelvin Scheduler Tool",
                vec![PluginCapability::ToolProvider, PluginCapability::FsWrite],
                "Local scheduler registry tool with explicit mutation approval.",
            ),
            tool: Arc::new(SchedulerTool {
                policy: policy.clone(),
            }),
        },
        SingleToolPlugin {
            manifest: manifest(
                "kelvin.tool.session",
                "Kelvin Session Tool",
                vec![
                    PluginCapability::ToolProvider,
                    PluginCapability::FsRead,
                    PluginCapability::FsWrite,
                ],
                "Session-local note/state helper with explicit clear controls.",
            ),
            tool: Arc::new(SessionToolsTool { policy }),
        },
    ];

    for plugin in plugins {
        registry.register(Arc::new(plugin), core_version, &registration_policy)?;
    }

    let projected = SdkToolRegistry::from_plugin_registry(&registry)?;
    let count = projected.names().len();
    Ok((Arc::new(projected), count))
}

#[cfg(test)]
mod tests {
    use super::load_default_toolpack_plugins;

    #[test]
    fn default_toolpack_projects_expected_tools() {
        let (registry, count) = load_default_toolpack_plugins("0.1.0").expect("toolpack");
        assert_eq!(count, 5);
        assert_eq!(
            registry.names(),
            vec![
                "fs_safe_read".to_string(),
                "fs_safe_write".to_string(),
                "schedule_cron".to_string(),
                "session_tools".to_string(),
                "web_fetch_safe".to_string()
            ]
        );
    }
}
