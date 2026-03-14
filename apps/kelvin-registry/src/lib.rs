use std::cmp::Ordering;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use kelvin_core::now_ms;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub bind_addr: SocketAddr,
    pub index_path: PathBuf,
    pub trust_policy_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIndex {
    pub schema_version: String,
    pub plugins: Vec<PluginIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIndexEntry {
    pub id: String,
    pub version: String,
    pub package_url: String,
    pub sha256: String,
    pub trust_policy_url: Option<String>,
    pub quality_tier: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
struct RegistryState {
    index_path: PathBuf,
    trust_policy_path: Option<PathBuf>,
    index: PluginIndex,
    trust_policy: Option<Value>,
    started_at_ms: u128,
}

#[derive(Debug, Default, Deserialize)]
struct PluginsQuery {
    id: Option<String>,
    tag: Option<String>,
    quality_tier: Option<String>,
    latest_only: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct PluginVersionsQuery {
    version: Option<String>,
}

pub fn router(config: RegistryConfig) -> Result<Router, String> {
    let state = Arc::new(load_state(config)?);
    Ok(Router::new()
        .route("/healthz", get(health))
        .route("/v1/index.json", get(index))
        .route("/v1/plugins", get(plugins))
        .route("/v1/plugins/{plugin_id}", get(plugin_versions))
        .route("/v1/trust-policy", get(trust_policy))
        .with_state(state))
}

pub async fn run_registry(config: RegistryConfig) -> Result<(), String> {
    let app = router(config.clone())?;
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .map_err(|err| {
            format!(
                "bind registry listener failed on {}: {err}",
                config.bind_addr
            )
        })?;
    axum::serve(listener, app)
        .await
        .map_err(|err| format!("serve registry failed: {err}"))
}

fn load_state(config: RegistryConfig) -> Result<RegistryState, String> {
    let index_bytes = fs::read(&config.index_path).map_err(|err| {
        format!(
            "read registry index '{}': {err}",
            config.index_path.to_string_lossy()
        )
    })?;
    let index: PluginIndex = serde_json::from_slice(&index_bytes).map_err(|err| {
        format!(
            "invalid registry index JSON '{}': {err}",
            config.index_path.to_string_lossy()
        )
    })?;
    if index.schema_version.trim() != "v1" {
        return Err(format!(
            "unsupported registry schema_version '{}' in '{}'",
            index.schema_version.trim(),
            config.index_path.to_string_lossy()
        ));
    }
    let trust_policy = match &config.trust_policy_path {
        Some(path) if path.is_file() => {
            let bytes = fs::read(path)
                .map_err(|err| format!("read trust policy '{}': {err}", path.to_string_lossy()))?;
            Some(serde_json::from_slice(&bytes).map_err(|err| {
                format!(
                    "invalid trust policy JSON '{}': {err}",
                    path.to_string_lossy()
                )
            })?)
        }
        Some(path) => {
            return Err(format!(
                "registry trust policy file does not exist: {}",
                path.to_string_lossy()
            ));
        }
        None => None,
    };
    Ok(RegistryState {
        index_path: config.index_path,
        trust_policy_path: config.trust_policy_path,
        index,
        trust_policy,
        started_at_ms: now_ms(),
    })
}

async fn health(State(state): State<Arc<RegistryState>>) -> Json<Value> {
    let plugin_ids = state
        .index
        .plugins
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    Json(json!({
        "ok": true,
        "index_path": state.index_path,
        "schema_version": state.index.schema_version,
        "plugin_count": state.index.plugins.len(),
        "unique_plugin_ids": plugin_ids.len(),
        "trust_policy_available": state.trust_policy.is_some(),
        "started_at_ms": state.started_at_ms,
    }))
}

async fn index(State(state): State<Arc<RegistryState>>) -> Json<PluginIndex> {
    Json(state.index.clone())
}

async fn plugins(
    State(state): State<Arc<RegistryState>>,
    Query(query): Query<PluginsQuery>,
) -> Json<Value> {
    let mut plugins = state.index.plugins.clone();
    if let Some(id) = query
        .id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        plugins.retain(|entry| entry.id == id);
    }
    if let Some(tag) = query
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        plugins.retain(|entry| entry.tags.iter().any(|value| value == tag));
    }
    if let Some(quality_tier) = query
        .quality_tier
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        plugins.retain(|entry| entry.quality_tier.as_deref() == Some(quality_tier));
    }
    sort_entries(&mut plugins);
    if query.latest_only.unwrap_or(false) {
        plugins = latest_entries(&plugins);
    }
    Json(json!({
        "schema_version": state.index.schema_version,
        "count": plugins.len(),
        "filters": {
            "id": query.id,
            "tag": query.tag,
            "quality_tier": query.quality_tier,
            "latest_only": query.latest_only.unwrap_or(false),
        },
        "plugins": plugins,
    }))
}

async fn plugin_versions(
    State(state): State<Arc<RegistryState>>,
    Path(plugin_id): Path<String>,
    Query(query): Query<PluginVersionsQuery>,
) -> Response {
    let plugin_id = plugin_id.trim();
    let mut versions = state
        .index
        .plugins
        .iter()
        .filter(|entry| entry.id == plugin_id)
        .cloned()
        .collect::<Vec<_>>();
    if versions.is_empty() {
        return error_response(
            StatusCode::NOT_FOUND,
            format!("plugin '{}' was not found in the registry index", plugin_id),
        );
    }
    sort_entries(&mut versions);
    if let Some(version) = query
        .version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let Some(entry) = versions
            .iter()
            .find(|entry| entry.version == version)
            .cloned()
        else {
            return error_response(
                StatusCode::NOT_FOUND,
                format!("plugin '{}' version '{}' was not found", plugin_id, version),
            );
        };
        return Json(json!({
            "plugin_id": plugin_id,
            "entry": entry,
        }))
        .into_response();
    }
    Json(json!({
        "plugin_id": plugin_id,
        "latest": versions.first().cloned(),
        "versions": versions,
    }))
    .into_response()
}

async fn trust_policy(State(state): State<Arc<RegistryState>>) -> Response {
    match (&state.trust_policy_path, &state.trust_policy) {
        (Some(path), Some(value)) => Json(json!({
            "path": path,
            "trust_policy": value,
        }))
        .into_response(),
        _ => error_response(
            StatusCode::NOT_FOUND,
            "registry trust policy is not configured",
        ),
    }
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message.into(),
            }
        })),
    )
        .into_response()
}

fn sort_entries(entries: &mut [PluginIndexEntry]) {
    entries.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| compare_versions_desc(&left.version, &right.version))
            .then_with(|| left.package_url.cmp(&right.package_url))
    });
}

fn latest_entries(entries: &[PluginIndexEntry]) -> Vec<PluginIndexEntry> {
    let mut out = Vec::new();
    let mut last_id = String::new();
    for entry in entries {
        if entry.id != last_id {
            out.push(entry.clone());
            last_id = entry.id.clone();
        }
    }
    out
}

fn compare_versions_desc(left: &str, right: &str) -> Ordering {
    match (Version::parse(left), Version::parse(right)) {
        (Ok(left), Ok(right)) => right.cmp(&left),
        _ => right.cmp(left),
    }
}
