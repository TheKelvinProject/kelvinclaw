use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use kelvin_brain::{default_plugin_home, default_trust_policy_path};
use kelvin_core::{KelvinError, SessionDescriptor, SessionMessage};
use kelvin_sdk::KelvinSdkRuntime;
use serde::Deserialize;
use serde_json::{json, Value};

const DEFAULT_PLUGIN_INDEX_URL: &str =
    "https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json";
const RUN_LIST_LIMIT_DEFAULT: usize = 25;
const RUN_LIST_LIMIT_MAX: usize = 200;
const SESSION_LIST_LIMIT_DEFAULT: usize = 25;
const SESSION_LIST_LIMIT_MAX: usize = 200;
const SESSION_MESSAGE_LIMIT_DEFAULT: usize = 20;
const SESSION_MESSAGE_LIMIT_MAX: usize = 200;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct OperatorRunsListParams {
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct OperatorSessionsListParams {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OperatorSessionGetParams {
    pub session_id: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct OperatorPluginsInspectParams {}

pub(crate) fn runs_list_payload(
    runtime: &KelvinSdkRuntime,
    params: OperatorRunsListParams,
) -> Result<Value, KelvinError> {
    let state_dir = runtime.state_dir().map(Path::to_path_buf);
    let Some(state_dir) = state_dir else {
        return Ok(json!({
            "enabled": false,
            "state_dir": null,
            "run_count": 0,
            "runs": [],
        }));
    };
    let runs_dir = state_dir.join("runs");
    let limit = params
        .limit
        .unwrap_or(RUN_LIST_LIMIT_DEFAULT)
        .clamp(1, RUN_LIST_LIMIT_MAX);
    let mut runs = Vec::new();
    if runs_dir.is_dir() {
        for entry in fs::read_dir(&runs_dir).map_err(|err| {
            KelvinError::Io(format!("read runs dir '{}': {err}", runs_dir.display()))
        })? {
            let entry =
                entry.map_err(|err| KelvinError::Io(format!("read runs dir entry: {err}")))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let bytes = fs::read(&path).map_err(|err| {
                KelvinError::Io(format!("read run record '{}': {err}", path.display()))
            })?;
            let value: Value = serde_json::from_slice(&bytes).map_err(|err| {
                KelvinError::InvalidInput(format!(
                    "invalid run record JSON '{}': {err}",
                    path.display()
                ))
            })?;
            runs.push(value);
        }
    }
    runs.sort_by_key(|item| {
        std::cmp::Reverse(
            item.get("updated_at_ms")
                .and_then(Value::as_u64)
                .or_else(|| item.get("accepted_at_ms").and_then(Value::as_u64))
                .unwrap_or_default(),
        )
    });
    runs.truncate(limit);
    Ok(json!({
        "enabled": true,
        "state_dir": state_dir,
        "run_count": runs.len(),
        "runs": runs,
    }))
}

pub(crate) fn sessions_list_payload(
    runtime: &KelvinSdkRuntime,
    params: OperatorSessionsListParams,
) -> Result<Value, KelvinError> {
    let state_dir = runtime.state_dir().map(Path::to_path_buf);
    let Some(state_dir) = state_dir else {
        return Ok(json!({
            "enabled": false,
            "state_dir": null,
            "session_count": 0,
            "sessions": [],
        }));
    };
    let sessions_dir = state_dir.join("sessions");
    let limit = params
        .limit
        .unwrap_or(SESSION_LIST_LIMIT_DEFAULT)
        .clamp(1, SESSION_LIST_LIMIT_MAX);
    let mut sessions = Vec::new();
    if sessions_dir.is_dir() {
        for entry in fs::read_dir(&sessions_dir).map_err(|err| {
            KelvinError::Io(format!(
                "read sessions dir '{}': {err}",
                sessions_dir.display()
            ))
        })? {
            let entry =
                entry.map_err(|err| KelvinError::Io(format!("read session entry: {err}")))?;
            let session_dir = entry.path();
            if !session_dir.is_dir() {
                continue;
            }
            let descriptor = match read_session_descriptor(&session_dir)? {
                Some(value) => value,
                None => continue,
            };
            let messages = read_session_messages(&session_dir)?;
            let last_message = messages.last().cloned();
            sessions.push(json!({
                "session_id": descriptor.session_id,
                "session_key": descriptor.session_key,
                "workspace_dir": descriptor.workspace_dir,
                "message_count": messages.len(),
                "last_message": last_message,
            }));
        }
    }
    sessions.sort_by_key(|item| {
        std::cmp::Reverse(
            item.get("last_message")
                .and_then(|value| value.get("metadata"))
                .and_then(|value| value.get("ts_ms"))
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        )
    });
    sessions.truncate(limit);
    Ok(json!({
        "enabled": true,
        "state_dir": state_dir,
        "session_count": sessions.len(),
        "sessions": sessions,
    }))
}

pub(crate) fn session_get_payload(
    runtime: &KelvinSdkRuntime,
    params: OperatorSessionGetParams,
) -> Result<Value, KelvinError> {
    let state_dir = runtime.state_dir().map(Path::to_path_buf);
    let Some(state_dir) = state_dir else {
        return Ok(json!({
            "enabled": false,
            "state_dir": null,
            "found": false,
        }));
    };
    let sessions_dir = state_dir.join("sessions");
    let limit = params
        .limit
        .unwrap_or(SESSION_MESSAGE_LIMIT_DEFAULT)
        .clamp(1, SESSION_MESSAGE_LIMIT_MAX);
    if !sessions_dir.is_dir() {
        return Ok(json!({
            "enabled": true,
            "state_dir": state_dir,
            "found": false,
            "session_id": params.session_id,
        }));
    }

    for entry in fs::read_dir(&sessions_dir).map_err(|err| {
        KelvinError::Io(format!(
            "read sessions dir '{}': {err}",
            sessions_dir.display()
        ))
    })? {
        let entry = entry.map_err(|err| KelvinError::Io(format!("read session entry: {err}")))?;
        let session_dir = entry.path();
        if !session_dir.is_dir() {
            continue;
        }
        let descriptor = match read_session_descriptor(&session_dir)? {
            Some(value) => value,
            None => continue,
        };
        if descriptor.session_id != params.session_id {
            continue;
        }
        let messages = read_session_messages(&session_dir)?;
        let message_count = messages.len();
        let messages = messages
            .into_iter()
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        return Ok(json!({
            "enabled": true,
            "found": true,
            "state_dir": state_dir,
            "descriptor": descriptor,
            "message_count": message_count,
            "messages": messages,
        }));
    }

    Ok(json!({
        "enabled": true,
        "state_dir": state_dir,
        "found": false,
        "session_id": params.session_id,
    }))
}

pub(crate) fn plugins_summary_payload(runtime: &KelvinSdkRuntime) -> Value {
    let plugin_home = default_plugin_home().ok();
    let trust_policy_path = default_trust_policy_path().ok();
    let scan = scan_plugin_home(plugin_home.as_deref()).unwrap_or_else(|err| PluginScan {
        plugins: Vec::new(),
        capability_usage: BTreeMap::new(),
        quality_tiers: BTreeMap::new(),
        publishers: BTreeMap::new(),
        current_versions: 0,
        signatures_present: 0,
        scan_error: Some(err.to_string()),
    });
    let trust = read_trust_policy_summary(trust_policy_path.as_deref());
    json!({
        "loaded_installed_plugins": runtime.loaded_installed_plugins(),
        "plugin_home": plugin_home,
        "plugin_home_exists": plugin_home.as_ref().is_some_and(|path| path.is_dir()),
        "trust_policy_path": trust_policy_path,
        "trust_policy": trust,
        "registry": registry_config_payload(),
        "capability_usage": scan.capability_usage,
        "quality_tiers": scan.quality_tiers,
        "publishers": scan.publishers,
        "audit_counters": {
            "plugin_count": scan.plugins.len(),
            "current_versions": scan.current_versions,
            "signatures_present": scan.signatures_present,
            "scan_error": scan.scan_error,
        },
    })
}

pub(crate) fn plugins_inspect_payload(
    runtime: &KelvinSdkRuntime,
    _params: OperatorPluginsInspectParams,
) -> Result<Value, KelvinError> {
    let plugin_home = default_plugin_home()?;
    let trust_policy_path = default_trust_policy_path()?;
    let scan = scan_plugin_home(Some(&plugin_home))?;
    Ok(json!({
        "loaded_installed_plugins": runtime.loaded_installed_plugins(),
        "plugin_home": plugin_home,
        "plugin_home_exists": plugin_home.is_dir(),
        "plugins": scan.plugins,
        "capability_usage": scan.capability_usage,
        "quality_tiers": scan.quality_tiers,
        "publishers": scan.publishers,
        "audit_counters": {
            "plugin_count": scan.plugins.len(),
            "current_versions": scan.current_versions,
            "signatures_present": scan.signatures_present,
            "scan_error": scan.scan_error,
        },
        "trust_policy_path": trust_policy_path,
        "trust_policy": read_trust_policy_summary(Some(&trust_policy_path)),
        "registry": registry_config_payload(),
    }))
}

fn registry_config_payload() -> Value {
    let registry_url = std::env::var("KELVIN_PLUGIN_REGISTRY_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let index_url = std::env::var("KELVIN_PLUGIN_INDEX_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_PLUGIN_INDEX_URL.to_string());
    json!({
        "registry_url": registry_url,
        "index_url": index_url,
    })
}

fn read_session_descriptor(path: &Path) -> Result<Option<SessionDescriptor>, KelvinError> {
    let descriptor_path = path.join("descriptor.json");
    if !descriptor_path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&descriptor_path).map_err(|err| {
        KelvinError::Io(format!(
            "read session descriptor '{}': {err}",
            descriptor_path.display()
        ))
    })?;
    let descriptor = serde_json::from_slice(&bytes).map_err(|err| {
        KelvinError::InvalidInput(format!(
            "invalid session descriptor JSON '{}': {err}",
            descriptor_path.display()
        ))
    })?;
    Ok(Some(descriptor))
}

fn read_session_messages(path: &Path) -> Result<Vec<SessionMessage>, KelvinError> {
    let messages_path = path.join("messages.jsonl");
    if !messages_path.is_file() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&messages_path).map_err(|err| {
        KelvinError::Io(format!(
            "open session messages '{}': {err}",
            messages_path.display()
        ))
    })?;
    let reader = BufReader::new(file);
    let mut messages = Vec::new();
    for (line_number, line) in reader.lines().enumerate() {
        let line = line.map_err(|err| {
            KelvinError::Io(format!(
                "read session messages '{}': line {}: {err}",
                messages_path.display(),
                line_number.saturating_add(1)
            ))
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let message = serde_json::from_str(&line).map_err(|err| {
            KelvinError::InvalidInput(format!(
                "invalid session message JSON '{}': line {}: {err}",
                messages_path.display(),
                line_number.saturating_add(1)
            ))
        })?;
        messages.push(message);
    }
    Ok(messages)
}

#[derive(Default)]
struct PluginScan {
    plugins: Vec<Value>,
    capability_usage: BTreeMap<String, usize>,
    quality_tiers: BTreeMap<String, usize>,
    publishers: BTreeMap<String, usize>,
    current_versions: usize,
    signatures_present: usize,
    scan_error: Option<String>,
}

fn scan_plugin_home(plugin_home: Option<&Path>) -> Result<PluginScan, KelvinError> {
    let Some(plugin_home) = plugin_home else {
        return Ok(PluginScan::default());
    };
    if !plugin_home.is_dir() {
        return Ok(PluginScan::default());
    }

    let mut scan = PluginScan::default();
    for entry in fs::read_dir(plugin_home).map_err(|err| {
        KelvinError::Io(format!(
            "read plugin home '{}': {err}",
            plugin_home.display()
        ))
    })? {
        let entry = entry.map_err(|err| KelvinError::Io(format!("read plugin entry: {err}")))?;
        let plugin_dir = entry.path();
        if !plugin_dir.is_dir() {
            continue;
        }
        let current = fs::read_link(plugin_dir.join("current"))
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|value| value.to_string_lossy().to_string())
            });
        for version_entry in fs::read_dir(&plugin_dir).map_err(|err| {
            KelvinError::Io(format!("read plugin dir '{}': {err}", plugin_dir.display()))
        })? {
            let version_entry = version_entry
                .map_err(|err| KelvinError::Io(format!("read plugin version: {err}")))?;
            let version_dir = version_entry.path();
            if !version_dir.is_dir() {
                continue;
            }
            let version = version_dir
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default();
            if version == "current" {
                continue;
            }
            let manifest_path = version_dir.join("plugin.json");
            if !manifest_path.is_file() {
                continue;
            }
            let bytes = fs::read(&manifest_path).map_err(|err| {
                KelvinError::Io(format!(
                    "read plugin manifest '{}': {err}",
                    manifest_path.display()
                ))
            })?;
            let manifest: Value = serde_json::from_slice(&bytes).map_err(|err| {
                KelvinError::InvalidInput(format!(
                    "invalid plugin manifest JSON '{}': {err}",
                    manifest_path.display()
                ))
            })?;
            let capabilities = manifest
                .get("capabilities")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for capability in &capabilities {
                if let Some(name) = capability.as_str() {
                    *scan.capability_usage.entry(name.to_string()).or_default() += 1;
                }
            }
            let quality_tier = manifest
                .get("quality_tier")
                .and_then(Value::as_str)
                .unwrap_or("unsigned_local")
                .to_string();
            *scan.quality_tiers.entry(quality_tier.clone()).or_default() += 1;
            if let Some(publisher) = manifest.get("publisher").and_then(Value::as_str) {
                *scan.publishers.entry(publisher.to_string()).or_default() += 1;
            }
            let is_current = current.as_deref() == Some(version.as_str());
            if is_current {
                scan.current_versions += 1;
            }
            let signature_present = version_dir.join("plugin.sig").is_file();
            if signature_present {
                scan.signatures_present += 1;
            }
            scan.plugins.push(json!({
                "id": manifest.get("id").cloned().unwrap_or(Value::Null),
                "name": manifest.get("name").cloned().unwrap_or(Value::Null),
                "version": manifest.get("version").cloned().unwrap_or_else(|| json!(version)),
                "runtime": manifest.get("runtime").cloned().unwrap_or_else(|| json!("wasm_tool_v1")),
                "publisher": manifest.get("publisher").cloned().unwrap_or(Value::Null),
                "quality_tier": quality_tier,
                "tool_name": manifest.get("tool_name").cloned().unwrap_or(Value::Null),
                "provider_name": manifest.get("provider_name").cloned().unwrap_or(Value::Null),
                "provider_profile": manifest.get("provider_profile").cloned().unwrap_or(Value::Null),
                "model_name": manifest.get("model_name").cloned().unwrap_or(Value::Null),
                "capabilities": capabilities,
                "signature_present": signature_present,
                "is_current": is_current,
                "manifest_path": manifest_path,
            }));
        }
    }
    scan.plugins.sort_by(|left, right| {
        let left_id = left.get("id").and_then(Value::as_str).unwrap_or_default();
        let right_id = right.get("id").and_then(Value::as_str).unwrap_or_default();
        let left_version = left
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let right_version = right
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        left_id
            .cmp(right_id)
            .then_with(|| left_version.cmp(right_version))
    });
    Ok(scan)
}

fn read_trust_policy_summary(path: Option<&Path>) -> Value {
    let Some(path) = path else {
        return json!({
            "exists": false,
            "ok": false,
            "error": "trust policy path is unavailable",
        });
    };
    if !path.is_file() {
        return json!({
            "exists": false,
            "ok": false,
            "path": path,
            "error": "trust policy file does not exist",
        });
    }
    let bytes = match fs::read(path) {
        Ok(value) => value,
        Err(err) => {
            return json!({
                "exists": true,
                "ok": false,
                "path": path,
                "error": format!("read trust policy failed: {err}"),
            });
        }
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(err) => {
            return json!({
                "exists": true,
                "ok": false,
                "path": path,
                "error": format!("invalid trust policy JSON: {err}"),
            });
        }
    };
    let publishers = value
        .get("publishers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let revoked = value
        .get("revoked_publishers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let pinned = value
        .get("pinned_plugin_publishers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    json!({
        "exists": true,
        "ok": true,
        "path": path,
        "require_signature": value.get("require_signature").and_then(Value::as_bool).unwrap_or(true),
        "publishers_total": publishers.len(),
        "revoked_total": revoked.len(),
        "pinned_total": pinned.len(),
        "publishers": publishers,
        "revoked_publishers": revoked,
        "pinned_plugin_publishers": pinned,
    })
}
