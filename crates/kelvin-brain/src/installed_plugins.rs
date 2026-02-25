use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio::time;
use wasmparser::{Parser, Payload};

use kelvin_core::{
    InMemoryPluginRegistry, KelvinError, KelvinResult, ModelInput, ModelOutput, ModelProvider,
    PluginCapability, PluginFactory, PluginManifest, PluginRegistry, PluginSecurityPolicy,
    SdkModelProviderRegistry, SdkToolRegistry, Tool, ToolCallInput, ToolCallResult,
};
use kelvin_wasm::{
    model_abi, ClawCall, ModelSandboxPolicy, SandboxPolicy, WasmModelHost, WasmSkillHost,
};

const DEFAULT_TOOL_RUNTIME_KIND: &str = "wasm_tool_v1";
const DEFAULT_MODEL_RUNTIME_KIND: &str = "wasm_model_v1";
const DEFAULT_TIMEOUT_MS: u64 = 2_000;
const DEFAULT_MAX_RETRIES: u32 = 0;
const DEFAULT_MAX_CALLS_PER_MINUTE: usize = 120;
const DEFAULT_CIRCUIT_BREAKER_FAILURES: u32 = 3;
const DEFAULT_CIRCUIT_BREAKER_COOLDOWN_MS: u64 = 30_000;
const DEFAULT_PLUGIN_HOME_RELATIVE: &str = ".kelvinclaw/plugins";
const DEFAULT_TRUST_POLICY_RELATIVE: &str = ".kelvinclaw/trusted_publishers.json";

#[derive(Debug, Clone)]
pub struct LoadedInstalledPlugin {
    pub id: String,
    pub version: String,
    pub tool_name: Option<String>,
    pub provider_name: Option<String>,
    pub model_name: Option<String>,
    pub runtime: String,
    pub publisher: Option<String>,
}

#[derive(Clone)]
pub struct LoadedInstalledPlugins {
    pub plugin_registry: Arc<InMemoryPluginRegistry>,
    pub tool_registry: Arc<SdkToolRegistry>,
    pub model_registry: Arc<SdkModelProviderRegistry>,
    pub loaded_plugins: Vec<LoadedInstalledPlugin>,
}

#[derive(Debug, Clone)]
pub struct InstalledPluginLoaderConfig {
    pub plugin_home: PathBuf,
    pub core_version: String,
    pub security_policy: PluginSecurityPolicy,
    pub trust_policy: PublisherTrustPolicy,
}

impl InstalledPluginLoaderConfig {
    pub fn new(plugin_home: impl Into<PathBuf>) -> Self {
        Self {
            plugin_home: plugin_home.into(),
            core_version: "0.1.0".to_string(),
            security_policy: PluginSecurityPolicy::default(),
            trust_policy: PublisherTrustPolicy::default(),
        }
    }
}

pub fn default_plugin_home() -> KelvinResult<PathBuf> {
    if let Some(path) = env_path("KELVIN_PLUGIN_HOME") {
        return Ok(path);
    }
    Ok(resolve_home_dir()?.join(DEFAULT_PLUGIN_HOME_RELATIVE))
}

pub fn default_trust_policy_path() -> KelvinResult<PathBuf> {
    if let Some(path) = env_path("KELVIN_TRUST_POLICY_PATH") {
        return Ok(path);
    }
    Ok(resolve_home_dir()?.join(DEFAULT_TRUST_POLICY_RELATIVE))
}

pub fn load_installed_tool_plugins_default(
    core_version: impl Into<String>,
    security_policy: PluginSecurityPolicy,
) -> KelvinResult<LoadedInstalledPlugins> {
    load_installed_plugins_default(core_version, security_policy)
}

pub fn load_installed_plugins_default(
    core_version: impl Into<String>,
    security_policy: PluginSecurityPolicy,
) -> KelvinResult<LoadedInstalledPlugins> {
    let trust_policy_path = default_trust_policy_path()?;
    let trust_policy = if let Some(path) = maybe_load_trust_policy_path(&trust_policy_path)? {
        PublisherTrustPolicy::from_json_file(path)?
    } else {
        PublisherTrustPolicy::default()
    };

    load_installed_plugins(InstalledPluginLoaderConfig {
        plugin_home: default_plugin_home()?,
        core_version: core_version.into(),
        security_policy,
        trust_policy,
    })
}

#[derive(Debug, Clone, Default)]
pub struct CapabilityScopes {
    pub fs_read_paths: Vec<String>,
    pub network_allow_hosts: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OperationalControls {
    pub timeout_ms: u64,
    pub max_retries: u32,
    pub max_calls_per_minute: usize,
    pub circuit_breaker_failures: u32,
    pub circuit_breaker_cooldown_ms: u64,
}

impl Default for OperationalControls {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_retries: DEFAULT_MAX_RETRIES,
            max_calls_per_minute: DEFAULT_MAX_CALLS_PER_MINUTE,
            circuit_breaker_failures: DEFAULT_CIRCUIT_BREAKER_FAILURES,
            circuit_breaker_cooldown_ms: DEFAULT_CIRCUIT_BREAKER_COOLDOWN_MS,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PublisherTrustPolicy {
    pub require_signature: bool,
    trusted_publishers: HashMap<String, VerifyingKey>,
}

impl Default for PublisherTrustPolicy {
    fn default() -> Self {
        Self {
            require_signature: true,
            trusted_publishers: HashMap::new(),
        }
    }
}

impl PublisherTrustPolicy {
    pub fn with_signature_requirement(mut self, required: bool) -> Self {
        self.require_signature = required;
        self
    }

    pub fn with_publisher_key(
        mut self,
        publisher_id: &str,
        ed25519_public_key_base64: &str,
    ) -> KelvinResult<Self> {
        let key = parse_public_key(ed25519_public_key_base64)?;
        self.trusted_publishers
            .insert(publisher_id.to_string(), key);
        Ok(self)
    }

    pub fn from_json_file(path: impl AsRef<Path>) -> KelvinResult<Self> {
        let text = fs::read_to_string(path.as_ref())?;
        let parsed: PublisherTrustPolicyFile = serde_json::from_str(&text).map_err(|err| {
            KelvinError::InvalidInput(format!("invalid publisher trust policy JSON: {err}"))
        })?;

        let mut policy = Self {
            require_signature: parsed.require_signature.unwrap_or(true),
            trusted_publishers: HashMap::new(),
        };
        for publisher in parsed.publishers {
            let key = parse_public_key(&publisher.ed25519_public_key)?;
            policy.trusted_publishers.insert(publisher.id, key);
        }
        Ok(policy)
    }

    fn verify_manifest_signature(
        &self,
        manifest: &InstalledPluginPackageManifest,
        manifest_bytes: &[u8],
        version_dir: &Path,
    ) -> KelvinResult<()> {
        let signature_path = version_dir.join("plugin.sig");
        let has_signature = signature_path.is_file();

        if !self.require_signature && !has_signature {
            return Ok(());
        }

        let publisher = manifest.publisher.as_deref().ok_or_else(|| {
            KelvinError::InvalidInput(format!(
                "plugin '{}' is missing publisher id for signature verification",
                manifest.id
            ))
        })?;
        let verifier = self.trusted_publishers.get(publisher).ok_or_else(|| {
            KelvinError::InvalidInput(format!(
                "plugin '{}' publisher '{}' is not trusted",
                manifest.id, publisher
            ))
        })?;

        if !has_signature {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' is missing required plugin.sig",
                manifest.id
            )));
        }

        let signature_text = fs::read_to_string(&signature_path)?;
        let signature_base64 = signature_text.trim();
        if signature_base64.is_empty() {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' has empty plugin.sig",
                manifest.id
            )));
        }
        let signature_bytes = STANDARD.decode(signature_base64).map_err(|err| {
            KelvinError::InvalidInput(format!("invalid plugin.sig base64: {err}"))
        })?;
        let signature = Signature::from_slice(&signature_bytes).map_err(|err| {
            KelvinError::InvalidInput(format!("invalid ed25519 signature: {err}"))
        })?;

        verifier.verify(manifest_bytes, &signature).map_err(|err| {
            KelvinError::InvalidInput(format!(
                "plugin '{}' signature verification failed: {err}",
                manifest.id
            ))
        })?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct PublisherTrustPolicyFile {
    #[serde(default)]
    require_signature: Option<bool>,
    #[serde(default)]
    publishers: Vec<TrustedPublisherEntry>,
}

#[derive(Debug, Deserialize)]
struct TrustedPublisherEntry {
    id: String,
    ed25519_public_key: String,
}

#[derive(Debug, Clone, Deserialize)]
struct InstalledPluginPackageManifest {
    id: String,
    name: String,
    version: String,
    api_version: String,
    description: Option<String>,
    homepage: Option<String>,
    #[serde(default)]
    capabilities: Vec<PluginCapability>,
    #[serde(default)]
    experimental: bool,
    min_core_version: Option<String>,
    max_core_version: Option<String>,
    runtime: Option<String>,
    tool_name: Option<String>,
    provider_name: Option<String>,
    model_name: Option<String>,
    entrypoint: String,
    entrypoint_sha256: Option<String>,
    publisher: Option<String>,
    #[serde(default)]
    capability_scopes: CapabilityScopesManifest,
    #[serde(default)]
    operational_controls: OperationalControlsManifest,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct CapabilityScopesManifest {
    #[serde(default)]
    fs_read_paths: Vec<String>,
    #[serde(default)]
    network_allow_hosts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OperationalControlsManifest {
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_max_retries")]
    max_retries: u32,
    #[serde(default = "default_max_calls_per_minute")]
    max_calls_per_minute: usize,
    #[serde(default = "default_circuit_breaker_failures")]
    circuit_breaker_failures: u32,
    #[serde(default = "default_circuit_breaker_cooldown_ms")]
    circuit_breaker_cooldown_ms: u64,
}

impl Default for OperationalControlsManifest {
    fn default() -> Self {
        Self {
            timeout_ms: default_timeout_ms(),
            max_retries: default_max_retries(),
            max_calls_per_minute: default_max_calls_per_minute(),
            circuit_breaker_failures: default_circuit_breaker_failures(),
            circuit_breaker_cooldown_ms: default_circuit_breaker_cooldown_ms(),
        }
    }
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

fn default_max_retries() -> u32 {
    DEFAULT_MAX_RETRIES
}

fn default_max_calls_per_minute() -> usize {
    DEFAULT_MAX_CALLS_PER_MINUTE
}

fn default_circuit_breaker_failures() -> u32 {
    DEFAULT_CIRCUIT_BREAKER_FAILURES
}

fn default_circuit_breaker_cooldown_ms() -> u64 {
    DEFAULT_CIRCUIT_BREAKER_COOLDOWN_MS
}

impl InstalledPluginPackageManifest {
    fn to_core_manifest(&self) -> PluginManifest {
        PluginManifest {
            id: self.id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            api_version: self.api_version.clone(),
            description: self.description.clone(),
            homepage: self.homepage.clone(),
            capabilities: self.capabilities.clone(),
            experimental: self.experimental,
            min_core_version: self.min_core_version.clone(),
            max_core_version: self.max_core_version.clone(),
        }
    }

    fn runtime_kind(&self) -> &str {
        self.runtime
            .as_deref()
            .unwrap_or(DEFAULT_TOOL_RUNTIME_KIND)
            .trim()
    }

    fn resolved_tool_name(&self) -> KelvinResult<String> {
        let fallback = self.id.replace('.', "_");
        let candidate = self
            .tool_name
            .as_deref()
            .unwrap_or(&fallback)
            .trim()
            .to_string();
        if candidate.is_empty() {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' has empty tool_name",
                self.id
            )));
        }
        if !candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' has invalid tool_name '{}'",
                self.id, candidate
            )));
        }
        Ok(candidate)
    }

    fn resolved_provider_name(&self) -> KelvinResult<String> {
        let fallback = self.id.replace('.', "_");
        let candidate = self
            .provider_name
            .as_deref()
            .unwrap_or(&fallback)
            .trim()
            .to_string();
        if candidate.is_empty() {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' has empty provider_name",
                self.id
            )));
        }
        if !candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' has invalid provider_name '{}'",
                self.id, candidate
            )));
        }
        Ok(candidate)
    }

    fn resolved_model_name(&self) -> KelvinResult<String> {
        let fallback = "default";
        let candidate = self
            .model_name
            .as_deref()
            .unwrap_or(fallback)
            .trim()
            .to_string();
        if candidate.is_empty() {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' has empty model_name",
                self.id
            )));
        }
        Ok(candidate)
    }
}

#[derive(Debug, Default)]
struct OperationalState {
    call_timestamps: VecDeque<Instant>,
    consecutive_failures: u32,
    circuit_open_until: Option<Instant>,
}

#[derive(Clone)]
struct InstalledWasmTool {
    plugin_id: String,
    plugin_version: String,
    tool_name: String,
    entrypoint_abs: PathBuf,
    host: Arc<WasmSkillHost>,
    sandbox_policy: SandboxPolicy,
    scopes: CapabilityScopes,
    controls: OperationalControls,
    state: Arc<Mutex<OperationalState>>,
}

impl InstalledWasmTool {
    #[allow(clippy::too_many_arguments)]
    fn new(
        plugin_id: String,
        plugin_version: String,
        tool_name: String,
        entrypoint_abs: PathBuf,
        host: Arc<WasmSkillHost>,
        sandbox_policy: SandboxPolicy,
        scopes: CapabilityScopes,
        controls: OperationalControls,
    ) -> Self {
        Self {
            plugin_id,
            plugin_version,
            tool_name,
            entrypoint_abs,
            host,
            sandbox_policy,
            scopes,
            controls,
            state: Arc::new(Mutex::new(OperationalState::default())),
        }
    }

    fn enforce_scoped_arguments(&self, args: &serde_json::Value) -> KelvinResult<()> {
        if self.sandbox_policy.allow_fs_read {
            let target_path = args
                .get("target_path")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    KelvinError::InvalidInput(format!(
                        "tool '{}' requires string argument 'target_path' when fs_read is enabled",
                        self.tool_name
                    ))
                })?;
            let normalized = normalize_safe_relative_path(target_path, "target_path")?;
            if !scope_match(&normalized, &self.scopes.fs_read_paths) {
                return Err(KelvinError::InvalidInput(format!(
                    "tool '{}' denied target_path '{}' (outside allowed fs_read scopes)",
                    self.tool_name, normalized
                )));
            }
        }

        if self.sandbox_policy.allow_network_send {
            let target_host = args
                .get("target_host")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    KelvinError::InvalidInput(format!(
                        "tool '{}' requires string argument 'target_host' when network is enabled",
                        self.tool_name
                    ))
                })?;
            if !host_allowed(target_host, &self.scopes.network_allow_hosts) {
                return Err(KelvinError::InvalidInput(format!(
                    "tool '{}' denied target_host '{}' (outside network allowlist)",
                    self.tool_name, target_host
                )));
            }
        }

        Ok(())
    }

    async fn reserve_call_budget(&self) -> KelvinResult<()> {
        let now = Instant::now();
        let mut state = self.state.lock().await;

        if let Some(open_until) = state.circuit_open_until {
            if now < open_until {
                return Err(KelvinError::Backend(format!(
                    "tool '{}' circuit breaker is open; retry later",
                    self.tool_name
                )));
            }
            state.circuit_open_until = None;
            state.consecutive_failures = 0;
        }

        let window = Duration::from_secs(60);
        while let Some(ts) = state.call_timestamps.front() {
            if now.duration_since(*ts) > window {
                state.call_timestamps.pop_front();
            } else {
                break;
            }
        }

        if state.call_timestamps.len() >= self.controls.max_calls_per_minute {
            return Err(KelvinError::Timeout(format!(
                "tool '{}' exceeded call budget ({} calls/minute)",
                self.tool_name, self.controls.max_calls_per_minute
            )));
        }
        state.call_timestamps.push_back(now);
        Ok(())
    }

    async fn mark_success(&self) {
        let mut state = self.state.lock().await;
        state.consecutive_failures = 0;
    }

    async fn mark_failure(&self) {
        let mut state = self.state.lock().await;
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.consecutive_failures >= self.controls.circuit_breaker_failures {
            state.circuit_open_until = Some(
                Instant::now() + Duration::from_millis(self.controls.circuit_breaker_cooldown_ms),
            );
            state.consecutive_failures = 0;
        }
    }

    async fn execute_once(&self) -> KelvinResult<kelvin_wasm::SkillExecution> {
        let host = self.host.clone();
        let entrypoint = self.entrypoint_abs.clone();
        let policy = self.sandbox_policy;

        let mut task = tokio::task::spawn_blocking(move || host.run_file(entrypoint, policy));
        match time::timeout(Duration::from_millis(self.controls.timeout_ms), &mut task).await {
            Ok(join_result) => join_result
                .map_err(|err| KelvinError::Backend(format!("tool task join failure: {err}")))?,
            Err(_) => {
                task.abort();
                Err(KelvinError::Timeout(format!(
                    "tool '{}' timed out after {}ms",
                    self.tool_name, self.controls.timeout_ms
                )))
            }
        }
    }
}

#[derive(Clone)]
struct InstalledWasmModelProvider {
    plugin_id: String,
    plugin_version: String,
    provider_name: String,
    model_name: String,
    entrypoint_abs: PathBuf,
    host: Arc<WasmModelHost>,
    scopes: CapabilityScopes,
    controls: OperationalControls,
    state: Arc<Mutex<OperationalState>>,
}

impl InstalledWasmModelProvider {
    #[allow(clippy::too_many_arguments)]
    fn new(
        plugin_id: String,
        plugin_version: String,
        provider_name: String,
        model_name: String,
        entrypoint_abs: PathBuf,
        host: Arc<WasmModelHost>,
        scopes: CapabilityScopes,
        controls: OperationalControls,
    ) -> Self {
        Self {
            plugin_id,
            plugin_version,
            provider_name,
            model_name,
            entrypoint_abs,
            host,
            scopes,
            controls,
            state: Arc::new(Mutex::new(OperationalState::default())),
        }
    }

    fn sandbox_policy(&self) -> ModelSandboxPolicy {
        ModelSandboxPolicy {
            network_allow_hosts: self.scopes.network_allow_hosts.clone(),
            timeout_ms: self.controls.timeout_ms,
            ..ModelSandboxPolicy::default()
        }
    }

    async fn reserve_call_budget(&self) -> KelvinResult<()> {
        let now = Instant::now();
        let mut state = self.state.lock().await;

        if let Some(open_until) = state.circuit_open_until {
            if now < open_until {
                return Err(KelvinError::Backend(format!(
                    "model provider '{}:{}' circuit breaker is open; retry later",
                    self.provider_name, self.model_name
                )));
            }
            state.circuit_open_until = None;
            state.consecutive_failures = 0;
        }

        let window = Duration::from_secs(60);
        while let Some(ts) = state.call_timestamps.front() {
            if now.duration_since(*ts) > window {
                state.call_timestamps.pop_front();
            } else {
                break;
            }
        }

        if state.call_timestamps.len() >= self.controls.max_calls_per_minute {
            return Err(KelvinError::Timeout(format!(
                "model provider '{}:{}' exceeded call budget ({} calls/minute)",
                self.provider_name, self.model_name, self.controls.max_calls_per_minute
            )));
        }
        state.call_timestamps.push_back(now);
        Ok(())
    }

    async fn mark_success(&self) {
        let mut state = self.state.lock().await;
        state.consecutive_failures = 0;
    }

    async fn mark_failure(&self) {
        let mut state = self.state.lock().await;
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.consecutive_failures >= self.controls.circuit_breaker_failures {
            state.circuit_open_until = Some(
                Instant::now() + Duration::from_millis(self.controls.circuit_breaker_cooldown_ms),
            );
            state.consecutive_failures = 0;
        }
    }

    async fn execute_once(&self, input_json: String) -> KelvinResult<String> {
        let host = self.host.clone();
        let entrypoint = self.entrypoint_abs.clone();
        let policy = self.sandbox_policy();

        let mut task =
            tokio::task::spawn_blocking(move || host.run_file(entrypoint, &input_json, policy));
        match time::timeout(Duration::from_millis(self.controls.timeout_ms), &mut task).await {
            Ok(join_result) => join_result.map_err(|err| {
                KelvinError::Backend(format!("model provider task join failure: {err}"))
            })?,
            Err(_) => {
                task.abort();
                Err(KelvinError::Timeout(format!(
                    "model provider '{}:{}' timed out after {}ms",
                    self.provider_name, self.model_name, self.controls.timeout_ms
                )))
            }
        }
    }

    fn decode_output_payload(&self, output_json: &str) -> KelvinResult<ModelOutput> {
        let value: Value = serde_json::from_str(output_json).map_err(|err| {
            KelvinError::InvalidInput(format!(
                "model plugin '{}' returned invalid json: {err}",
                self.plugin_id
            ))
        })?;
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|message| message.as_str())
        {
            return Err(KelvinError::Backend(format!(
                "model plugin '{}@{}' failed: {}",
                self.plugin_id, self.plugin_version, message
            )));
        }
        serde_json::from_value::<ModelOutput>(value).map_err(|err| {
            KelvinError::InvalidInput(format!(
                "model plugin '{}' returned invalid model output: {err}",
                self.plugin_id
            ))
        })
    }
}

#[async_trait]
impl Tool for InstalledWasmTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    async fn call(&self, input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        self.enforce_scoped_arguments(&input.arguments)?;
        self.reserve_call_budget().await?;

        let mut last_error = None;
        for attempt in 0..=self.controls.max_retries {
            match self.execute_once().await {
                Ok(execution) => {
                    self.mark_success().await;
                    let summary = format!(
                        "{} executed exit={} calls={} plugin={}@{}",
                        self.tool_name,
                        execution.exit_code,
                        execution.calls.len(),
                        self.plugin_id,
                        self.plugin_version
                    );
                    let calls = execution
                        .calls
                        .iter()
                        .map(claw_call_json)
                        .collect::<Vec<_>>();
                    let output = json!({
                        "plugin_id": self.plugin_id,
                        "plugin_version": self.plugin_version,
                        "entrypoint": self.entrypoint_abs.to_string_lossy(),
                        "exit_code": execution.exit_code,
                        "calls": calls,
                    });
                    return Ok(ToolCallResult {
                        summary: summary.clone(),
                        output: Some(output.to_string()),
                        visible_text: Some(summary),
                        is_error: false,
                    });
                }
                Err(err) => {
                    last_error = Some(err);
                    if attempt < self.controls.max_retries {
                        continue;
                    }
                }
            }
        }

        self.mark_failure().await;
        Err(last_error.unwrap_or_else(|| {
            KelvinError::Backend(format!(
                "tool '{}' failed without error detail",
                self.tool_name
            ))
        }))
    }
}

#[async_trait]
impl ModelProvider for InstalledWasmModelProvider {
    fn provider_name(&self) -> &str {
        &self.provider_name
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    async fn infer(&self, input: ModelInput) -> KelvinResult<ModelOutput> {
        self.reserve_call_budget().await?;
        let input_json = serde_json::to_string(&input).map_err(|err| {
            KelvinError::InvalidInput(format!(
                "serialize model input for plugin '{}': {err}",
                self.plugin_id
            ))
        })?;

        let mut last_error = None;
        for attempt in 0..=self.controls.max_retries {
            match self.execute_once(input_json.clone()).await {
                Ok(output_json) => {
                    let output = self.decode_output_payload(&output_json)?;
                    self.mark_success().await;
                    return Ok(output);
                }
                Err(err) => {
                    last_error = Some(err);
                    if attempt < self.controls.max_retries {
                        continue;
                    }
                }
            }
        }

        self.mark_failure().await;
        Err(last_error.unwrap_or_else(|| {
            KelvinError::Backend(format!(
                "model provider '{}:{}' failed without error detail",
                self.provider_name, self.model_name
            ))
        }))
    }
}

struct InstalledWasmPluginFactory {
    manifest: PluginManifest,
    tool: Option<Arc<InstalledWasmTool>>,
    model_provider: Option<Arc<InstalledWasmModelProvider>>,
}

impl PluginFactory for InstalledWasmPluginFactory {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn tool(&self) -> Option<Arc<dyn Tool>> {
        self.tool.clone().map(|tool| tool as Arc<dyn Tool>)
    }

    fn model_provider(&self) -> Option<Arc<dyn ModelProvider>> {
        self.model_provider
            .clone()
            .map(|provider| provider as Arc<dyn ModelProvider>)
    }
}

pub fn load_installed_tool_plugins(
    config: InstalledPluginLoaderConfig,
) -> KelvinResult<LoadedInstalledPlugins> {
    load_installed_plugins(config)
}

pub fn load_installed_plugins(
    config: InstalledPluginLoaderConfig,
) -> KelvinResult<LoadedInstalledPlugins> {
    let plugin_registry = Arc::new(InMemoryPluginRegistry::new());
    let mut loaded_plugins = Vec::new();

    if !config.plugin_home.exists() {
        let tool_registry = Arc::new(SdkToolRegistry::from_plugin_registry(
            plugin_registry.as_ref(),
        )?);
        let model_registry = Arc::new(SdkModelProviderRegistry::from_plugin_registry(
            plugin_registry.as_ref(),
        )?);
        return Ok(LoadedInstalledPlugins {
            plugin_registry,
            tool_registry,
            model_registry,
            loaded_plugins,
        });
    }

    let plugin_dirs = collect_plugin_dirs(&config.plugin_home)?;
    let skill_host = Arc::new(WasmSkillHost::try_new()?);
    let model_host = Arc::new(WasmModelHost::try_new()?);
    for plugin_dir in plugin_dirs {
        let plugin = load_one_plugin(&plugin_dir, &config, skill_host.clone(), model_host.clone())?;

        let loaded = LoadedInstalledPlugin {
            id: plugin.manifest.id.clone(),
            version: plugin.manifest.version.clone(),
            tool_name: plugin.tool.as_ref().map(|tool| tool.name().to_string()),
            provider_name: plugin
                .model_provider
                .as_ref()
                .map(|provider| provider.provider_name.clone()),
            model_name: plugin
                .model_provider
                .as_ref()
                .map(|provider| provider.model_name.clone()),
            runtime: plugin.runtime.clone(),
            publisher: plugin.publisher.clone(),
        };

        plugin_registry.register(
            Arc::new(InstalledWasmPluginFactory {
                manifest: plugin.manifest,
                tool: plugin.tool,
                model_provider: plugin.model_provider,
            }),
            &config.core_version,
            &config.security_policy,
        )?;
        loaded_plugins.push(loaded);
    }

    loaded_plugins.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| left.version.cmp(&right.version))
            .then_with(|| left.runtime.cmp(&right.runtime))
            .then_with(|| left.tool_name.cmp(&right.tool_name))
            .then_with(|| left.provider_name.cmp(&right.provider_name))
            .then_with(|| left.model_name.cmp(&right.model_name))
    });

    let tool_registry = Arc::new(SdkToolRegistry::from_plugin_registry(
        plugin_registry.as_ref(),
    )?);
    let model_registry = Arc::new(SdkModelProviderRegistry::from_plugin_registry(
        plugin_registry.as_ref(),
    )?);
    Ok(LoadedInstalledPlugins {
        plugin_registry,
        tool_registry,
        model_registry,
        loaded_plugins,
    })
}

struct LoadedPluginFactoryData {
    manifest: PluginManifest,
    tool: Option<Arc<InstalledWasmTool>>,
    model_provider: Option<Arc<InstalledWasmModelProvider>>,
    runtime: String,
    publisher: Option<String>,
}

fn load_one_plugin(
    plugin_dir: &Path,
    config: &InstalledPluginLoaderConfig,
    skill_host: Arc<WasmSkillHost>,
    model_host: Arc<WasmModelHost>,
) -> KelvinResult<LoadedPluginFactoryData> {
    let plugin_id = plugin_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| KelvinError::InvalidInput("invalid plugin directory name".to_string()))?;
    let version_dir = resolve_version_dir(plugin_dir)?;

    let manifest_path = version_dir.join("plugin.json");
    let manifest_bytes = fs::read(&manifest_path)?;
    let package_manifest: InstalledPluginPackageManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|err| {
            KelvinError::InvalidInput(format!(
                "invalid plugin manifest at {}: {err}",
                manifest_path.to_string_lossy()
            ))
        })?;

    if package_manifest.id != plugin_id {
        return Err(KelvinError::InvalidInput(format!(
            "plugin id mismatch: directory '{}' but manifest id '{}'",
            plugin_id, package_manifest.id
        )));
    }

    let runtime_kind = package_manifest.runtime_kind();
    if runtime_kind != DEFAULT_TOOL_RUNTIME_KIND && runtime_kind != DEFAULT_MODEL_RUNTIME_KIND {
        return Err(KelvinError::InvalidInput(format!(
            "unsupported plugin runtime '{}'; expected '{}' or '{}'",
            runtime_kind, DEFAULT_TOOL_RUNTIME_KIND, DEFAULT_MODEL_RUNTIME_KIND
        )));
    }

    config.trust_policy.verify_manifest_signature(
        &package_manifest,
        &manifest_bytes,
        &version_dir,
    )?;

    let core_manifest = package_manifest.to_core_manifest();
    core_manifest.validate()?;
    let entrypoint_rel = normalize_safe_relative_path(&package_manifest.entrypoint, "entrypoint")?;
    let entrypoint_abs = version_dir.join("payload").join(&entrypoint_rel);
    if !entrypoint_abs.is_file() {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' entrypoint file not found: payload/{}",
            package_manifest.id, entrypoint_rel
        )));
    }

    if let Some(expected_sha) = package_manifest.entrypoint_sha256.as_deref() {
        let entrypoint_bytes = fs::read(&entrypoint_abs)?;
        let actual_sha = sha256_hex(&entrypoint_bytes);
        if !actual_sha.eq_ignore_ascii_case(expected_sha.trim()) {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' entrypoint_sha256 mismatch",
                package_manifest.id
            )));
        }
    }

    let scopes = normalize_scopes(&package_manifest)?;
    let controls = normalize_controls(&package_manifest)?;
    let mut tool = None;
    let mut model_provider = None;

    if runtime_kind == DEFAULT_TOOL_RUNTIME_KIND {
        if !package_manifest
            .capabilities
            .contains(&PluginCapability::ToolProvider)
        {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' runtime '{}' requires capability '{}'",
                package_manifest.id, DEFAULT_TOOL_RUNTIME_KIND, "tool_provider"
            )));
        }

        if package_manifest
            .capabilities
            .contains(&PluginCapability::FsWrite)
        {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' declares unsupported capability 'fs_write' for runtime '{}'",
                package_manifest.id, DEFAULT_TOOL_RUNTIME_KIND
            )));
        }

        if package_manifest
            .capabilities
            .contains(&PluginCapability::CommandExecution)
        {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' declares unsupported capability 'command_execution' for runtime '{}'",
                package_manifest.id, DEFAULT_TOOL_RUNTIME_KIND
            )));
        }

        let tool_name = package_manifest.resolved_tool_name()?;
        let sandbox_policy = sandbox_from_manifest(&package_manifest)?;
        tool = Some(Arc::new(InstalledWasmTool::new(
            package_manifest.id.clone(),
            package_manifest.version.clone(),
            tool_name,
            entrypoint_abs.clone(),
            skill_host,
            sandbox_policy,
            scopes.clone(),
            controls.clone(),
        )));
    } else {
        if !package_manifest
            .capabilities
            .contains(&PluginCapability::ModelProvider)
        {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' runtime '{}' requires capability '{}'",
                package_manifest.id, DEFAULT_MODEL_RUNTIME_KIND, "model_provider"
            )));
        }
        if package_manifest
            .capabilities
            .contains(&PluginCapability::FsRead)
            || package_manifest
                .capabilities
                .contains(&PluginCapability::FsWrite)
            || package_manifest
                .capabilities
                .contains(&PluginCapability::CommandExecution)
        {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' runtime '{}' only supports model_provider and optional network_egress capabilities",
                package_manifest.id, DEFAULT_MODEL_RUNTIME_KIND
            )));
        }

        let provider_name = package_manifest.resolved_provider_name()?;
        let model_name = package_manifest.resolved_model_name()?;
        model_provider = Some(Arc::new(InstalledWasmModelProvider::new(
            package_manifest.id.clone(),
            package_manifest.version.clone(),
            provider_name,
            model_name,
            entrypoint_abs.clone(),
            model_host,
            scopes.clone(),
            controls.clone(),
        )));

        let entrypoint_bytes = fs::read(&entrypoint_abs)?;
        validate_model_plugin_imports(&entrypoint_bytes, &package_manifest.id)?;
    }

    Ok(LoadedPluginFactoryData {
        manifest: core_manifest,
        tool,
        model_provider,
        runtime: runtime_kind.to_string(),
        publisher: package_manifest.publisher.clone(),
    })
}

fn collect_plugin_dirs(plugin_home: &Path) -> KelvinResult<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(plugin_home)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        dirs.push(entry.path());
    }
    dirs.sort();
    Ok(dirs)
}

fn resolve_version_dir(plugin_dir: &Path) -> KelvinResult<PathBuf> {
    let current = plugin_dir.join("current");
    if current.is_symlink() {
        let target = fs::read_link(&current)?;
        let target_str = target.to_string_lossy().to_string();
        let normalized = normalize_safe_relative_path(&target_str, "current symlink target")?;
        let resolved = plugin_dir.join(&normalized);
        if resolved.is_dir() {
            return Ok(resolved);
        }
        return Err(KelvinError::InvalidInput(format!(
            "plugin current symlink points to missing directory: {}",
            current.to_string_lossy()
        )));
    }

    let mut best: Option<(semver::Version, PathBuf)> = None;
    for entry in fs::read_dir(plugin_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir_name = entry.file_name();
        let dir_name = dir_name.to_string_lossy();
        if dir_name == "current" {
            continue;
        }
        let Ok(version) = semver::Version::parse(&dir_name) else {
            continue;
        };
        match &best {
            Some((best_version, _)) if version <= *best_version => {}
            _ => best = Some((version, entry.path())),
        }
    }

    best.map(|(_, path)| path).ok_or_else(|| {
        KelvinError::InvalidInput(format!(
            "plugin '{}' has no version directories",
            plugin_dir.to_string_lossy()
        ))
    })
}

fn normalize_safe_relative_path(raw: &str, field_name: &str) -> KelvinResult<String> {
    let normalized = raw.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{field_name} must not be empty"
        )));
    }
    if Path::new(&normalized).is_absolute() || normalized.starts_with('/') {
        return Err(KelvinError::InvalidInput(format!(
            "{field_name} must be relative path"
        )));
    }
    let path = Path::new(&normalized);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(KelvinError::InvalidInput(format!(
            "{field_name} path traversal is not allowed"
        )));
    }
    Ok(normalized)
}

fn normalize_scopes(manifest: &InstalledPluginPackageManifest) -> KelvinResult<CapabilityScopes> {
    let has_fs_read = manifest.capabilities.contains(&PluginCapability::FsRead);
    let has_network = manifest
        .capabilities
        .contains(&PluginCapability::NetworkEgress);
    let runtime_requires_network_scope = manifest.runtime_kind() == DEFAULT_MODEL_RUNTIME_KIND;
    let network_scope_required = has_network || runtime_requires_network_scope;

    let mut fs_read_paths = Vec::new();
    for path in &manifest.capability_scopes.fs_read_paths {
        fs_read_paths.push(normalize_safe_relative_path(
            path,
            "capability_scopes.fs_read_paths",
        )?);
    }
    if has_fs_read && fs_read_paths.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' declares fs_read but has no fs_read scope paths",
            manifest.id
        )));
    }
    if !has_fs_read && !fs_read_paths.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' has fs_read scope paths but does not declare fs_read capability",
            manifest.id
        )));
    }

    let mut network_allow_hosts = Vec::new();
    for host in &manifest.capability_scopes.network_allow_hosts {
        network_allow_hosts.push(normalize_host_pattern(host)?);
    }
    if network_scope_required && network_allow_hosts.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' requires network allowlist but has no network allow hosts",
            manifest.id
        )));
    }
    if !has_network && !runtime_requires_network_scope && !network_allow_hosts.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' has network allowlist but does not declare network_egress capability",
            manifest.id
        )));
    }

    Ok(CapabilityScopes {
        fs_read_paths,
        network_allow_hosts,
    })
}

fn normalize_controls(
    manifest: &InstalledPluginPackageManifest,
) -> KelvinResult<OperationalControls> {
    let controls = &manifest.operational_controls;
    if controls.timeout_ms == 0 || controls.timeout_ms > 120_000 {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' timeout_ms must be between 1 and 120000",
            manifest.id
        )));
    }
    if controls.max_retries > 5 {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' max_retries must be <= 5",
            manifest.id
        )));
    }
    if controls.max_calls_per_minute == 0 || controls.max_calls_per_minute > 10_000 {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' max_calls_per_minute must be between 1 and 10000",
            manifest.id
        )));
    }
    if controls.circuit_breaker_failures == 0 || controls.circuit_breaker_failures > 100 {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' circuit_breaker_failures must be between 1 and 100",
            manifest.id
        )));
    }
    if controls.circuit_breaker_cooldown_ms < 100 || controls.circuit_breaker_cooldown_ms > 600_000
    {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' circuit_breaker_cooldown_ms must be between 100 and 600000",
            manifest.id
        )));
    }

    Ok(OperationalControls {
        timeout_ms: controls.timeout_ms,
        max_retries: controls.max_retries,
        max_calls_per_minute: controls.max_calls_per_minute,
        circuit_breaker_failures: controls.circuit_breaker_failures,
        circuit_breaker_cooldown_ms: controls.circuit_breaker_cooldown_ms,
    })
}

fn sandbox_from_manifest(manifest: &InstalledPluginPackageManifest) -> KelvinResult<SandboxPolicy> {
    let mut policy = SandboxPolicy::locked_down();
    if manifest.capabilities.contains(&PluginCapability::FsRead) {
        policy.allow_fs_read = true;
    }
    if manifest
        .capabilities
        .contains(&PluginCapability::NetworkEgress)
    {
        policy.allow_network_send = true;
    }
    if manifest.capabilities.contains(&PluginCapability::FsWrite)
        || manifest
            .capabilities
            .contains(&PluginCapability::CommandExecution)
    {
        return Err(KelvinError::InvalidInput(format!(
            "plugin '{}' requests unsupported runtime capability",
            manifest.id
        )));
    }
    Ok(policy)
}

fn validate_model_plugin_imports(wasm_bytes: &[u8], plugin_id: &str) -> KelvinResult<()> {
    for payload in Parser::new(0).parse_all(wasm_bytes) {
        let payload = payload
            .map_err(|err| KelvinError::InvalidInput(format!("invalid model wasm: {err}")))?;
        if let Payload::ImportSection(section) = payload {
            for import in section {
                let import = import.map_err(|err| {
                    KelvinError::InvalidInput(format!("invalid model wasm import section: {err}"))
                })?;
                if import.module != model_abi::MODULE {
                    return Err(KelvinError::InvalidInput(format!(
                        "model plugin '{}' has forbidden import module '{}'",
                        plugin_id, import.module
                    )));
                }
                match import.name {
                    model_abi::IMPORT_OPENAI_RESPONSES_CALL
                    | model_abi::IMPORT_LOG
                    | model_abi::IMPORT_CLOCK_NOW_MS => {}
                    name => {
                        return Err(KelvinError::InvalidInput(format!(
                            "model plugin '{}' has forbidden import '{}.{}'",
                            plugin_id, import.module, name
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn parse_public_key(key_base64: &str) -> KelvinResult<VerifyingKey> {
    let bytes = STANDARD
        .decode(key_base64.trim())
        .map_err(|err| KelvinError::InvalidInput(format!("invalid base64 public key: {err}")))?;
    if bytes.len() != 32 {
        return Err(KelvinError::InvalidInput(
            "ed25519 public key must be 32 bytes".to_string(),
        ));
    }
    let mut key = [0_u8; 32];
    key.copy_from_slice(&bytes);
    VerifyingKey::from_bytes(&key)
        .map_err(|err| KelvinError::InvalidInput(format!("invalid ed25519 public key: {err}")))
}

fn normalize_host_pattern(input: &str) -> KelvinResult<String> {
    let cleaned = input.trim().to_ascii_lowercase();
    if cleaned.is_empty() {
        return Err(KelvinError::InvalidInput(
            "network allowlist host must not be empty".to_string(),
        ));
    }
    if cleaned
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(KelvinError::InvalidInput(
            "network allowlist host must not contain whitespace/control characters".to_string(),
        ));
    }
    if !cleaned
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '*' | ':'))
    {
        return Err(KelvinError::InvalidInput(format!(
            "invalid network allowlist host pattern: {cleaned}"
        )));
    }
    Ok(cleaned)
}

fn env_path(key: &str) -> Option<PathBuf> {
    let value = env::var(key).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

fn resolve_home_dir() -> KelvinResult<PathBuf> {
    env_path("HOME").ok_or_else(|| {
        KelvinError::InvalidInput(
            "HOME is not set; configure KELVIN_PLUGIN_HOME and KELVIN_TRUST_POLICY_PATH explicitly"
                .to_string(),
        )
    })
}

fn maybe_load_trust_policy_path(path: &Path) -> KelvinResult<Option<&Path>> {
    if path.exists() {
        return Ok(Some(path));
    }

    if env_path("KELVIN_TRUST_POLICY_PATH").is_some() {
        return Err(KelvinError::InvalidInput(format!(
            "configured trust policy file does not exist: {}",
            path.to_string_lossy()
        )));
    }

    Ok(None)
}

fn host_allowed(target: &str, allowlist: &[String]) -> bool {
    let candidate = target.trim().to_ascii_lowercase();
    allowlist.iter().any(|pattern| {
        if let Some(rest) = pattern.strip_prefix("*.") {
            candidate.ends_with(rest)
                && candidate.len() > rest.len()
                && candidate.as_bytes()[candidate.len() - rest.len() - 1] == b'.'
        } else {
            candidate == *pattern
        }
    })
}

fn scope_match(target: &str, allowlist: &[String]) -> bool {
    allowlist.iter().any(|scope| {
        target == scope
            || target
                .strip_prefix(scope)
                .map(|rest| rest.starts_with('/'))
                .unwrap_or(false)
    })
}

fn claw_call_json(call: &ClawCall) -> serde_json::Value {
    match call {
        ClawCall::SendMessage { message_code } => json!({
            "kind": "send_message",
            "message_code": message_code,
        }),
        ClawCall::MoveServo { channel, position } => json!({
            "kind": "move_servo",
            "channel": channel,
            "position": position,
        }),
        ClawCall::FsRead { handle } => json!({
            "kind": "fs_read",
            "handle": handle,
        }),
        ClawCall::NetworkSend { packet } => json!({
            "kind": "network_send",
            "packet": packet,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    use super::{
        load_installed_plugins, load_installed_tool_plugins, sha256_hex,
        InstalledPluginLoaderConfig, PublisherTrustPolicy,
    };
    use kelvin_core::{PluginSecurityPolicy, ToolCallInput, ToolRegistry};

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!("kelvin-installed-{name}-{millis}"));
        std::fs::create_dir_all(&path).expect("create temp directory");
        path
    }

    fn write_installed_plugin(
        plugin_home: &Path,
        plugin_id: &str,
        version: &str,
        manifest_value: serde_json::Value,
        wat_source: &str,
        signing_key: Option<&SigningKey>,
    ) {
        let version_dir = plugin_home.join(plugin_id).join(version);
        let payload_dir = version_dir.join("payload");
        std::fs::create_dir_all(&payload_dir).expect("create payload dir");

        let entrypoint_rel = manifest_value["entrypoint"]
            .as_str()
            .expect("entrypoint string");
        let wasm_bytes = wat::parse_str(wat_source).expect("compile wat");
        let entrypoint_abs = payload_dir.join(entrypoint_rel);
        std::fs::write(&entrypoint_abs, &wasm_bytes).expect("write wasm entrypoint");

        let mut manifest = manifest_value;
        if manifest["entrypoint_sha256"].is_null() {
            manifest["entrypoint_sha256"] = json!(sha256_hex(&wasm_bytes));
        }

        let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest bytes");
        std::fs::write(version_dir.join("plugin.json"), &manifest_bytes).expect("write manifest");

        if let Some(key) = signing_key {
            let signature = key.sign(&manifest_bytes);
            let signature_base64 =
                base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
            std::fs::write(version_dir.join("plugin.sig"), signature_base64)
                .expect("write signature");
        }
    }

    fn default_manifest(plugin_id: &str, version: &str) -> serde_json::Value {
        json!({
            "id": plugin_id,
            "name": "Installed Plugin",
            "version": version,
            "api_version": "1.0.0",
            "description": "installed runtime plugin",
            "homepage": "https://example.com/plugin",
            "capabilities": ["tool_provider"],
            "experimental": false,
            "runtime": "wasm_tool_v1",
            "tool_name": "installed_echo",
            "entrypoint": "echo.wasm",
            "entrypoint_sha256": null,
            "publisher": "acme",
            "capability_scopes": {
                "fs_read_paths": [],
                "network_allow_hosts": []
            },
            "operational_controls": {
                "timeout_ms": 2000,
                "max_retries": 0,
                "max_calls_per_minute": 100,
                "circuit_breaker_failures": 2,
                "circuit_breaker_cooldown_ms": 1000
            }
        })
    }

    fn default_model_manifest(plugin_id: &str, version: &str) -> serde_json::Value {
        json!({
            "id": plugin_id,
            "name": "Installed Model Plugin",
            "version": version,
            "api_version": "1.0.0",
            "description": "installed runtime model plugin",
            "homepage": "https://example.com/plugin",
            "capabilities": ["model_provider"],
            "experimental": false,
            "runtime": "wasm_model_v1",
            "provider_name": "openai",
            "model_name": "gpt-4.1-mini",
            "entrypoint": "model.wasm",
            "entrypoint_sha256": null,
            "publisher": "acme",
            "capability_scopes": {
                "fs_read_paths": [],
                "network_allow_hosts": ["api.openai.com"]
            },
            "operational_controls": {
                "timeout_ms": 2000,
                "max_retries": 0,
                "max_calls_per_minute": 100,
                "circuit_breaker_failures": 2,
                "circuit_breaker_cooldown_ms": 1000
            }
        })
    }

    #[tokio::test]
    async fn loads_signed_plugin_and_executes_tool() {
        let plugin_home = unique_temp_dir("load-exec");
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());

        write_installed_plugin(
            &plugin_home,
            "acme.echo",
            "1.0.0",
            default_manifest("acme.echo", "1.0.0"),
            r#"
            (module
              (import "claw" "send_message" (func $send_message (param i32) (result i32)))
              (func (export "run") (result i32)
                i32.const 55
                call $send_message
                drop
                i32.const 0
              )
            )
            "#,
            Some(&signing_key),
        );

        let trust_policy = PublisherTrustPolicy::default()
            .with_publisher_key("acme", &public_key)
            .expect("publisher key");
        let loaded = load_installed_tool_plugins(InstalledPluginLoaderConfig {
            plugin_home: plugin_home.clone(),
            core_version: "0.1.0".to_string(),
            security_policy: PluginSecurityPolicy::default(),
            trust_policy,
        })
        .expect("load installed plugin");

        assert_eq!(loaded.loaded_plugins.len(), 1);
        let tool = loaded
            .tool_registry
            .get("installed_echo")
            .expect("tool should be registered");
        let result = tool
            .call(ToolCallInput {
                run_id: "run-1".to_string(),
                session_id: "session-1".to_string(),
                workspace_dir: plugin_home.to_string_lossy().to_string(),
                arguments: json!({}),
            })
            .await
            .expect("tool call");
        assert!(!result.is_error);
        assert!(result.summary.contains("acme.echo@1.0.0"));
    }

    #[test]
    fn rejects_missing_signature_when_required() {
        let plugin_home = unique_temp_dir("missing-signature");
        let signing_key = SigningKey::from_bytes(&[8_u8; 32]);
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());
        write_installed_plugin(
            &plugin_home,
            "acme.echo",
            "1.0.0",
            default_manifest("acme.echo", "1.0.0"),
            r#"
            (module
              (func (export "run") (result i32)
                i32.const 0
              )
            )
            "#,
            None,
        );

        let trust_policy = PublisherTrustPolicy::default()
            .with_publisher_key("acme", &public_key)
            .expect("publisher key");
        let err = match load_installed_tool_plugins(InstalledPluginLoaderConfig {
            plugin_home: plugin_home.clone(),
            core_version: "0.1.0".to_string(),
            security_policy: PluginSecurityPolicy::default(),
            trust_policy,
        }) {
            Ok(_) => panic!("signature should be required"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("missing required plugin.sig"));
    }

    #[tokio::test]
    async fn enforces_scopes_and_operational_controls() {
        let plugin_home = unique_temp_dir("scopes-controls");
        let signing_key = SigningKey::from_bytes(&[9_u8; 32]);
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());

        let mut manifest = default_manifest("acme.scoped", "1.0.0");
        manifest["capabilities"] = json!(["tool_provider", "fs_read", "network_egress"]);
        manifest["capability_scopes"] = json!({
            "fs_read_paths": ["memory/allowed"],
            "network_allow_hosts": ["api.example.com"]
        });
        manifest["operational_controls"] = json!({
            "timeout_ms": 2000,
            "max_retries": 0,
            "max_calls_per_minute": 1,
            "circuit_breaker_failures": 1,
            "circuit_breaker_cooldown_ms": 5000
        });

        write_installed_plugin(
            &plugin_home,
            "acme.scoped",
            "1.0.0",
            manifest,
            r#"
            (module
              (import "claw" "fs_read" (func $fs_read (param i32) (result i32)))
              (import "claw" "network_send" (func $network_send (param i32) (result i32)))
              (func (export "run") (result i32)
                i32.const 1
                call $fs_read
                drop
                i32.const 2
                call $network_send
                drop
                i32.const 0
              )
            )
            "#,
            Some(&signing_key),
        );

        let trust_policy = PublisherTrustPolicy::default()
            .with_publisher_key("acme", &public_key)
            .expect("publisher key");
        let loaded = load_installed_tool_plugins(InstalledPluginLoaderConfig {
            plugin_home: plugin_home.clone(),
            core_version: "0.1.0".to_string(),
            security_policy: PluginSecurityPolicy {
                allow_fs_read: true,
                allow_network_egress: true,
                ..Default::default()
            },
            trust_policy,
        })
        .expect("load installed plugin");

        let tool = loaded
            .tool_registry
            .get("installed_echo")
            .expect("tool should be registered");

        let scope_err = tool
            .call(ToolCallInput {
                run_id: "run-scope".to_string(),
                session_id: "session-scope".to_string(),
                workspace_dir: plugin_home.to_string_lossy().to_string(),
                arguments: json!({
                    "target_path": "memory/blocked/file.md",
                    "target_host": "api.example.com"
                }),
            })
            .await
            .expect_err("scope should deny path");
        assert!(scope_err
            .to_string()
            .contains("outside allowed fs_read scopes"));

        let ok = tool
            .call(ToolCallInput {
                run_id: "run-ok".to_string(),
                session_id: "session-ok".to_string(),
                workspace_dir: plugin_home.to_string_lossy().to_string(),
                arguments: json!({
                    "target_path": "memory/allowed/file.md",
                    "target_host": "api.example.com"
                }),
            })
            .await
            .expect("first allowed call");
        assert!(!ok.is_error);

        let rate_err = tool
            .call(ToolCallInput {
                run_id: "run-rate".to_string(),
                session_id: "session-rate".to_string(),
                workspace_dir: plugin_home.to_string_lossy().to_string(),
                arguments: json!({
                    "target_path": "memory/allowed/file.md",
                    "target_host": "api.example.com"
                }),
            })
            .await
            .expect_err("rate limit should apply");
        assert!(rate_err.to_string().contains("exceeded call budget"));
    }

    #[test]
    fn loads_signed_model_plugin_and_projects_model_registry() {
        let plugin_home = unique_temp_dir("load-model");
        let signing_key = SigningKey::from_bytes(&[10_u8; 32]);
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());

        write_installed_plugin(
            &plugin_home,
            "acme.openai",
            "1.0.0",
            default_model_manifest("acme.openai", "1.0.0"),
            r#"
            (module
              (import "kelvin_model_host_v1" "openai_responses_call" (func $openai_responses_call (param i32 i32) (result i64)))
              (import "kelvin_model_host_v1" "log" (func $log (param i32 i32 i32) (result i32)))
              (import "kelvin_model_host_v1" "clock_now_ms" (func $clock_now_ms (result i64)))
              (memory (export "memory") 2)
              (global $heap (mut i32) (i32.const 1024))
              (func (export "alloc") (param $len i32) (result i32)
                (local $ptr i32)
                global.get $heap
                local.tee $ptr
                local.get $len
                i32.add
                global.set $heap
                local.get $ptr)
              (func (export "dealloc") (param i32 i32))
              (func (export "infer") (param $ptr i32) (param $len i32) (result i64)
                local.get $ptr
                local.get $len
                call $openai_responses_call)
            )
            "#,
            Some(&signing_key),
        );

        let trust_policy = PublisherTrustPolicy::default()
            .with_publisher_key("acme", &public_key)
            .expect("publisher key");
        let loaded = load_installed_plugins(InstalledPluginLoaderConfig {
            plugin_home,
            core_version: "0.1.0".to_string(),
            security_policy: PluginSecurityPolicy::default(),
            trust_policy,
        })
        .expect("load installed model plugin");

        assert_eq!(loaded.loaded_plugins.len(), 1);
        assert_eq!(
            loaded.loaded_plugins[0].provider_name.as_deref(),
            Some("openai")
        );
        assert_eq!(
            loaded.loaded_plugins[0].model_name.as_deref(),
            Some("gpt-4.1-mini")
        );
        let provider = loaded
            .model_registry
            .get_by_plugin_id("acme.openai")
            .expect("model registry entry");
        assert_eq!(provider.provider_name(), "openai");
        assert_eq!(provider.model_name(), "gpt-4.1-mini");
    }
}
