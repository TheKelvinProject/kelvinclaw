use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use semver::Version;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, RwLock};
use tokio::time::{sleep, Duration};
use uuid::Uuid;

use kelvin_brain::{load_installed_plugins_default, EchoModelProvider, KelvinBrain};
use kelvin_core::{
    now_ms, AgentEvent, AgentRunRequest, AgentWaitResult, CoreRuntime, EventSink, KelvinError,
    KelvinResult, MemorySearchManager, ModelInput, ModelOutput, ModelProvider,
    PluginSecurityPolicy, RunOutcome, RunState, SessionDescriptor, SessionMessage, SessionRole,
    SessionStore, Tool, ToolCallInput, ToolCallResult, ToolRegistry,
};
#[cfg(any(not(feature = "memory_rpc"), feature = "memory_legacy_fallback"))]
use kelvin_memory::MemoryBackendKind;
#[cfg(any(not(feature = "memory_rpc"), feature = "memory_legacy_fallback"))]
use kelvin_memory::MemoryFactory;
#[cfg(feature = "memory_rpc")]
use kelvin_memory_client::{MemoryClientConfig, RpcMemoryManager};

pub mod scheduler;
mod toolpack;

pub use scheduler::{
    ClaimedScheduleSlot, NewScheduledTask, ScheduleAuditEntry, ScheduleReplyDelivery,
    ScheduleReplyTarget, ScheduleSlotPhase, ScheduleSlotRecord, ScheduledTask, SchedulerStatus,
    SchedulerStore,
};

const MIN_DEFAULT_TIMEOUT_MS: u64 = 100;
const MAX_DEFAULT_TIMEOUT_MS: u64 = 300_000;
const MAX_CONFIG_ID_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KelvinCliMemoryMode {
    Markdown,
    InMemory,
    Fallback,
}

impl KelvinCliMemoryMode {
    pub fn parse(input: &str) -> Self {
        let normalized = input.trim().to_lowercase();
        match normalized.as_str() {
            "markdown" | "md" => Self::Markdown,
            "in-memory" | "in_memory" | "memory" | "inmemory" => Self::InMemory,
            "fallback" | "with-fallback" => Self::Fallback,
            _ => Self::Markdown,
        }
    }

    #[cfg(any(not(feature = "memory_rpc"), feature = "memory_legacy_fallback"))]
    fn as_backend_kind(self) -> MemoryBackendKind {
        match self {
            Self::Markdown => MemoryBackendKind::Markdown,
            Self::InMemory => MemoryBackendKind::InMemoryVector,
            Self::Fallback => MemoryBackendKind::InMemoryWithMarkdownFallback,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KelvinSdkConfig {
    pub prompt: String,
    pub session_id: String,
    pub workspace_dir: PathBuf,
    pub memory_mode: KelvinCliMemoryMode,
    pub timeout_ms: u64,
    pub system_prompt: Option<String>,
    pub core_version: String,
    pub plugin_security_policy: PluginSecurityPolicy,
    pub load_installed_plugins: bool,
    pub model_provider: KelvinSdkModelSelection,
    pub state_dir: Option<PathBuf>,
    pub persist_runs: bool,
    pub max_session_history_messages: usize,
    pub compact_to_messages: usize,
}

impl KelvinSdkConfig {
    pub fn validate(&self) -> KelvinResult<()> {
        if self.prompt.trim().is_empty() {
            return Err(KelvinError::InvalidInput(
                "prompt must not be empty".to_string(),
            ));
        }
        validate_config_identifier("session id", &self.session_id)?;
        validate_workspace_dir(&self.workspace_dir, "workspace_dir")?;
        validate_timeout_ms(self.timeout_ms, "timeout_ms")?;
        validate_core_version(&self.core_version)?;
        validate_model_selection(&self.model_provider, self.load_installed_plugins)?;
        validate_persistence_policy(self.max_session_history_messages, self.compact_to_messages)?;
        if let Some(state_dir) = &self.state_dir {
            validate_state_dir(state_dir)?;
        }
        Ok(())
    }

    pub fn for_prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            session_id: "main".to_string(),
            workspace_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            memory_mode: KelvinCliMemoryMode::Markdown,
            timeout_ms: 30_000,
            system_prompt: None,
            core_version: "0.1.0".to_string(),
            plugin_security_policy: PluginSecurityPolicy::default(),
            load_installed_plugins: true,
            model_provider: KelvinSdkModelSelection::Echo,
            state_dir: None,
            persist_runs: true,
            max_session_history_messages: 128,
            compact_to_messages: 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KelvinSdkModelSelection {
    Echo,
    InstalledPlugin {
        plugin_id: String,
    },
    InstalledPluginFailover {
        plugin_ids: Vec<String>,
        max_retries_per_provider: u8,
        retry_backoff_ms: u64,
    },
}

#[derive(Debug, Clone)]
pub struct KelvinRunSummary {
    pub run_id: String,
    pub accepted_at_ms: u128,
    pub provider: String,
    pub model: String,
    pub duration_ms: u128,
    pub payloads: Vec<String>,
    pub loaded_installed_plugins: usize,
    pub cli_plugin_preflight: String,
}

#[derive(Debug, Clone)]
pub struct KelvinSdkRuntimeConfig {
    pub workspace_dir: PathBuf,
    pub default_session_id: String,
    pub memory_mode: KelvinCliMemoryMode,
    pub default_timeout_ms: u64,
    pub default_system_prompt: Option<String>,
    pub core_version: String,
    pub plugin_security_policy: PluginSecurityPolicy,
    pub load_installed_plugins: bool,
    pub model_provider: KelvinSdkModelSelection,
    pub require_cli_plugin_tool: bool,
    pub emit_stdout_events: bool,
    pub state_dir: Option<PathBuf>,
    pub persist_runs: bool,
    pub max_session_history_messages: usize,
    pub compact_to_messages: usize,
}

impl KelvinSdkRuntimeConfig {
    pub fn validate(&self) -> KelvinResult<()> {
        validate_config_identifier("default session id", &self.default_session_id)?;
        validate_workspace_dir(&self.workspace_dir, "workspace_dir")?;
        validate_timeout_ms(self.default_timeout_ms, "default_timeout_ms")?;
        validate_core_version(&self.core_version)?;
        validate_model_selection(&self.model_provider, self.load_installed_plugins)?;
        validate_persistence_policy(self.max_session_history_messages, self.compact_to_messages)?;
        if let Some(state_dir) = &self.state_dir {
            validate_state_dir(state_dir)?;
        }
        Ok(())
    }

    pub fn from_run_config(config: &KelvinSdkConfig) -> Self {
        Self {
            workspace_dir: config.workspace_dir.clone(),
            default_session_id: config.session_id.clone(),
            memory_mode: config.memory_mode,
            default_timeout_ms: config.timeout_ms,
            default_system_prompt: config.system_prompt.clone(),
            core_version: config.core_version.clone(),
            plugin_security_policy: config.plugin_security_policy.clone(),
            load_installed_plugins: config.load_installed_plugins,
            model_provider: config.model_provider.clone(),
            require_cli_plugin_tool: true,
            emit_stdout_events: true,
            state_dir: config
                .state_dir
                .clone()
                .or_else(|| Some(config.workspace_dir.join(".kelvin").join("state"))),
            persist_runs: config.persist_runs,
            max_session_history_messages: config.max_session_history_messages,
            compact_to_messages: config.compact_to_messages,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KelvinSdkRunRequest {
    pub prompt: String,
    pub session_id: Option<String>,
    pub workspace_dir: Option<PathBuf>,
    pub timeout_ms: Option<u64>,
    pub system_prompt: Option<String>,
    pub memory_query: Option<String>,
    pub run_id: Option<String>,
}

impl KelvinSdkRunRequest {
    pub fn for_prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            session_id: None,
            workspace_dir: None,
            timeout_ms: None,
            system_prompt: None,
            memory_query: None,
            run_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KelvinSdkAcceptedRun {
    pub run_id: String,
    pub accepted_at_ms: u128,
    pub cli_plugin_preflight: Option<String>,
}

fn validate_workspace_dir(path: &Path, label: &str) -> KelvinResult<()> {
    if path.as_os_str().is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    if !path.exists() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} does not exist: {}",
            path.to_string_lossy()
        )));
    }
    if !path.is_dir() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must be a directory: {}",
            path.to_string_lossy()
        )));
    }
    Ok(())
}

fn validate_timeout_ms(value: u64, label: &str) -> KelvinResult<()> {
    if !(MIN_DEFAULT_TIMEOUT_MS..=MAX_DEFAULT_TIMEOUT_MS).contains(&value) {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must be between {MIN_DEFAULT_TIMEOUT_MS} and {MAX_DEFAULT_TIMEOUT_MS} milliseconds"
        )));
    }
    Ok(())
}

fn validate_persistence_policy(
    max_session_history_messages: usize,
    compact_to_messages: usize,
) -> KelvinResult<()> {
    if max_session_history_messages < 4 {
        return Err(KelvinError::InvalidInput(
            "max_session_history_messages must be >= 4".to_string(),
        ));
    }
    if compact_to_messages < 2 {
        return Err(KelvinError::InvalidInput(
            "compact_to_messages must be >= 2".to_string(),
        ));
    }
    if compact_to_messages >= max_session_history_messages {
        return Err(KelvinError::InvalidInput(
            "compact_to_messages must be smaller than max_session_history_messages".to_string(),
        ));
    }
    Ok(())
}

fn validate_state_dir(path: &Path) -> KelvinResult<()> {
    if path.as_os_str().is_empty() {
        return Err(KelvinError::InvalidInput(
            "state_dir must not be empty".to_string(),
        ));
    }
    if path.exists() && !path.is_dir() {
        return Err(KelvinError::InvalidInput(format!(
            "state_dir must be a directory when present: {}",
            path.to_string_lossy()
        )));
    }
    Ok(())
}

fn validate_core_version(value: &str) -> KelvinResult<()> {
    if value.trim().is_empty() {
        return Err(KelvinError::InvalidInput(
            "core_version must not be empty".to_string(),
        ));
    }
    Version::parse(value).map_err(|err| {
        KelvinError::InvalidInput(format!("core_version must be valid semver: {err}"))
    })?;
    Ok(())
}

fn validate_config_identifier(label: &str, value: &str) -> KelvinResult<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    if trimmed.chars().count() > MAX_CONFIG_ID_LEN {
        return Err(KelvinError::InvalidInput(format!(
            "{label} exceeds max length {MAX_CONFIG_ID_LEN}"
        )));
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not include control characters"
        )));
    }
    Ok(())
}

fn validate_model_selection(
    selection: &KelvinSdkModelSelection,
    load_installed_plugins: bool,
) -> KelvinResult<()> {
    match selection {
        KelvinSdkModelSelection::Echo => Ok(()),
        KelvinSdkModelSelection::InstalledPlugin { plugin_id } => {
            if !load_installed_plugins {
                return Err(KelvinError::InvalidInput(
                    "model provider plugin selection requires load_installed_plugins=true"
                        .to_string(),
                ));
            }
            validate_config_identifier("model provider plugin id", plugin_id)
        }
        KelvinSdkModelSelection::InstalledPluginFailover {
            plugin_ids,
            max_retries_per_provider: _,
            retry_backoff_ms,
        } => {
            if !load_installed_plugins {
                return Err(KelvinError::InvalidInput(
                    "model provider failover selection requires load_installed_plugins=true"
                        .to_string(),
                ));
            }
            if plugin_ids.is_empty() {
                return Err(KelvinError::InvalidInput(
                    "model failover requires at least one plugin id".to_string(),
                ));
            }
            let mut seen = std::collections::HashSet::<String>::new();
            for plugin_id in plugin_ids {
                validate_config_identifier("model provider plugin id", plugin_id)?;
                if !seen.insert(plugin_id.clone()) {
                    return Err(KelvinError::InvalidInput(format!(
                        "duplicate model provider plugin id in failover chain: {plugin_id}"
                    )));
                }
            }
            if *retry_backoff_ms == 0 {
                return Err(KelvinError::InvalidInput(
                    "retry_backoff_ms must be >= 1".to_string(),
                ));
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
struct RuntimePersistence {
    state_dir: Option<PathBuf>,
    persist_runs: bool,
    max_session_history_messages: usize,
    compact_to_messages: usize,
}

impl RuntimePersistence {
    fn new(
        state_dir: Option<PathBuf>,
        persist_runs: bool,
        max_session_history_messages: usize,
        compact_to_messages: usize,
    ) -> KelvinResult<Self> {
        validate_persistence_policy(max_session_history_messages, compact_to_messages)?;
        if let Some(dir) = &state_dir {
            validate_state_dir(dir)?;
        }

        let persistence = Self {
            state_dir,
            persist_runs,
            max_session_history_messages,
            compact_to_messages,
        };
        persistence.ensure_layout()?;
        Ok(persistence)
    }

    fn sessions_dir(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|root| root.join("sessions"))
    }

    fn runs_dir(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|root| root.join("runs"))
    }

    fn ensure_layout(&self) -> KelvinResult<()> {
        if let Some(root) = &self.state_dir {
            fs::create_dir_all(root.join("sessions"))
                .map_err(|err| KelvinError::Io(format!("create sessions dir: {err}")))?;
            fs::create_dir_all(root.join("runs"))
                .map_err(|err| KelvinError::Io(format!("create runs dir: {err}")))?;
        }
        Ok(())
    }

    fn key_hex(value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        let digest = hasher.finalize();
        let mut out = String::with_capacity(digest.len() * 2);
        for byte in digest {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    fn session_dir_for(&self, session_id: &str) -> Option<PathBuf> {
        self.sessions_dir()
            .map(|dir| dir.join(Self::key_hex(session_id)))
    }

    fn run_file_for(&self, run_id: &str) -> Option<PathBuf> {
        self.runs_dir()
            .map(|dir| dir.join(format!("{}.json", Self::key_hex(run_id))))
    }

    fn load_session_data(&self) -> KelvinResult<(SessionDescriptorMap, SessionMessagesMap)> {
        let mut sessions: SessionDescriptorMap = HashMap::new();
        let mut messages: SessionMessagesMap = HashMap::new();
        let Some(sessions_dir) = self.sessions_dir() else {
            return Ok((sessions, messages));
        };
        if !sessions_dir.is_dir() {
            return Ok((sessions, messages));
        }

        let entries = fs::read_dir(&sessions_dir)
            .map_err(|err| KelvinError::Io(format!("read sessions dir: {err}")))?;
        for entry in entries {
            let entry = match entry {
                Ok(value) => value,
                Err(err) => {
                    eprintln!("warning: skipping unreadable session entry: {err}");
                    continue;
                }
            };
            let is_dir = match entry.file_type() {
                Ok(file_type) => file_type.is_dir(),
                Err(err) => {
                    eprintln!(
                        "warning: skipping session entry with unreadable type '{}': {err}",
                        entry.path().to_string_lossy()
                    );
                    continue;
                }
            };
            if !is_dir {
                continue;
            }
            let descriptor_path = entry.path().join("descriptor.json");
            if !descriptor_path.is_file() {
                continue;
            }
            let descriptor_bytes = match fs::read(&descriptor_path) {
                Ok(bytes) => bytes,
                Err(err) => {
                    eprintln!(
                        "warning: failed to read descriptor '{}': {err}; continuing",
                        descriptor_path.to_string_lossy()
                    );
                    continue;
                }
            };
            let descriptor: SessionDescriptor = match serde_json::from_slice(&descriptor_bytes) {
                Ok(parsed) => parsed,
                Err(err) => {
                    quarantine_corrupt_file(
                        &descriptor_path,
                        &format!("invalid session descriptor json: {err}"),
                    );
                    continue;
                }
            };

            let messages_path = entry.path().join("messages.jsonl");
            let session_messages = if messages_path.is_file() {
                self.read_messages_file(&messages_path)
            } else {
                Vec::new()
            };

            messages.insert(descriptor.session_id.clone(), session_messages);
            sessions.insert(descriptor.session_id.clone(), descriptor);
        }

        Ok((sessions, messages))
    }

    fn read_messages_file(&self, path: &PathBuf) -> Vec<SessionMessage> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(err) => {
                quarantine_corrupt_file(path, &format!("unable to open messages file: {err}"));
                return Vec::new();
            }
        };
        let reader = BufReader::new(file);
        let mut out = Vec::new();
        for (line_number, line) in reader.lines().enumerate() {
            let line = match line {
                Ok(value) => value,
                Err(err) => {
                    quarantine_corrupt_file(
                        path,
                        &format!(
                            "failed to read messages line {}: {err}",
                            line_number.saturating_add(1)
                        ),
                    );
                    return Vec::new();
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let message: SessionMessage = match serde_json::from_str(&line) {
                Ok(parsed) => parsed,
                Err(err) => {
                    quarantine_corrupt_file(
                        path,
                        &format!(
                            "invalid messages line {} json: {err}",
                            line_number.saturating_add(1)
                        ),
                    );
                    return Vec::new();
                }
            };
            out.push(message);
        }
        out
    }

    fn persist_session_descriptor(&self, session: &SessionDescriptor) -> KelvinResult<()> {
        let Some(session_dir) = self.session_dir_for(&session.session_id) else {
            return Ok(());
        };
        fs::create_dir_all(&session_dir)
            .map_err(|err| KelvinError::Io(format!("create session dir: {err}")))?;
        let descriptor_path = session_dir.join("descriptor.json");
        let bytes = serde_json::to_vec_pretty(session)
            .map_err(|err| KelvinError::Io(format!("serialize descriptor: {err}")))?;
        write_atomic(&descriptor_path, &bytes)
    }

    fn persist_session_messages(
        &self,
        session_id: &str,
        messages: &[SessionMessage],
    ) -> KelvinResult<()> {
        let Some(session_dir) = self.session_dir_for(session_id) else {
            return Ok(());
        };
        fs::create_dir_all(&session_dir)
            .map_err(|err| KelvinError::Io(format!("create session dir: {err}")))?;
        let mut bytes = Vec::new();
        for message in messages {
            let line = serde_json::to_vec(message)
                .map_err(|err| KelvinError::Io(format!("serialize message: {err}")))?;
            bytes.extend_from_slice(&line);
            bytes.push(b'\n');
        }
        write_atomic(&session_dir.join("messages.jsonl"), &bytes)
    }

    fn persist_run_record(&self, run_id: &str, record: serde_json::Value) -> KelvinResult<()> {
        if !self.persist_runs {
            return Ok(());
        }
        let Some(path) = self.run_file_for(run_id) else {
            return Ok(());
        };

        let mut merged = if path.is_file() {
            let current = fs::read(&path)
                .map_err(|err| KelvinError::Io(format!("read run record: {err}")))?;
            serde_json::from_slice::<serde_json::Value>(&current).unwrap_or_else(|_| json!({}))
        } else {
            json!({})
        };

        if !merged.is_object() {
            merged = json!({});
        }
        if let Some(object) = merged.as_object_mut() {
            object.insert("run_id".to_string(), json!(run_id));
            object.insert("updated_at_ms".to_string(), json!(now_ms()));
            if let Some(incoming) = record.as_object() {
                for (key, value) in incoming {
                    object.insert(key.clone(), value.clone());
                }
            }
        }

        let bytes = serde_json::to_vec_pretty(&merged)
            .map_err(|err| KelvinError::Io(format!("serialize run record: {err}")))?;
        write_atomic(&path, &bytes)
    }
}

fn write_atomic(path: &PathBuf, bytes: &[u8]) -> KelvinResult<()> {
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
            "warning: corrupt state artifact could not be quarantined (invalid path '{}'): {}",
            path.to_string_lossy(),
            reason
        );
        return;
    };
    let renamed = path.with_file_name(format!("{file_name}.corrupt.{}", now_ms()));
    match fs::rename(path, &renamed) {
        Ok(()) => eprintln!(
            "warning: quarantined corrupt state artifact '{}' as '{}' ({})",
            path.to_string_lossy(),
            renamed.to_string_lossy(),
            reason
        ),
        Err(err) => eprintln!(
            "warning: corrupt state artifact '{}' detected but quarantine rename failed: {} ({})",
            path.to_string_lossy(),
            err,
            reason
        ),
    }
}

struct FileBackedSessionStore {
    sessions: RwLock<SessionDescriptorMap>,
    messages: RwLock<SessionMessagesMap>,
    persistence: RuntimePersistence,
}

impl FileBackedSessionStore {
    fn load(persistence: RuntimePersistence) -> KelvinResult<Self> {
        let (sessions, messages) = persistence.load_session_data()?;
        Ok(Self {
            sessions: RwLock::new(sessions),
            messages: RwLock::new(messages),
            persistence,
        })
    }

    fn compact_messages(&self, session_id: &str, messages: &mut Vec<SessionMessage>) {
        if messages.len() <= self.persistence.max_session_history_messages {
            return;
        }
        let drop_count = messages
            .len()
            .saturating_sub(self.persistence.compact_to_messages);
        let dropped_slice = &messages[..drop_count];
        let mut role_counts = HashMap::<String, usize>::new();
        for message in dropped_slice {
            let role = match message.role {
                SessionRole::User => "user",
                SessionRole::Assistant => "assistant",
                SessionRole::Tool => "tool",
                SessionRole::System => "system",
            };
            *role_counts.entry(role.to_string()).or_default() += 1;
        }

        let mut role_parts = role_counts
            .iter()
            .map(|(role, count)| format!("{role}:{count}"))
            .collect::<Vec<_>>();
        role_parts.sort();
        let summary = SessionMessage {
            role: SessionRole::System,
            content: format!(
                "history compacted for session '{session_id}'; dropped {drop_count} messages ({})",
                role_parts.join(",")
            ),
            metadata: json!({
                "compacted": true,
                "dropped_messages": drop_count,
                "policy": {
                    "max_session_history_messages": self.persistence.max_session_history_messages,
                    "compact_to_messages": self.persistence.compact_to_messages
                }
            }),
        };
        let mut tail = messages.split_off(drop_count);
        messages.clear();
        messages.push(summary);
        messages.append(&mut tail);
    }
}

#[async_trait]
impl SessionStore for FileBackedSessionStore {
    async fn upsert_session(&self, session: SessionDescriptor) -> KelvinResult<()> {
        self.sessions
            .write()
            .await
            .insert(session.session_id.clone(), session.clone());
        self.persistence.persist_session_descriptor(&session)
    }

    async fn get_session(&self, session_id: &str) -> KelvinResult<Option<SessionDescriptor>> {
        Ok(self.sessions.read().await.get(session_id).cloned())
    }

    async fn append_message(&self, session_id: &str, message: SessionMessage) -> KelvinResult<()> {
        let snapshot = {
            let mut messages = self.messages.write().await;
            let session_messages = messages.entry(session_id.to_string()).or_default();
            session_messages.push(message);
            self.compact_messages(session_id, session_messages);
            session_messages.clone()
        };
        self.persistence
            .persist_session_messages(session_id, &snapshot)?;
        Ok(())
    }

    async fn history(&self, session_id: &str) -> KelvinResult<Vec<SessionMessage>> {
        Ok(self
            .messages
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default())
    }
}

#[derive(Debug, Default)]
struct StdoutEventSink;

#[async_trait]
impl EventSink for StdoutEventSink {
    async fn emit(&self, event: AgentEvent) -> KelvinResult<()> {
        println!("{}", serde_json::to_string(&event).unwrap_or_default());
        Ok(())
    }
}

#[derive(Clone)]
struct MultiplexEventSink {
    emit_stdout: bool,
    broadcast_tx: broadcast::Sender<AgentEvent>,
}

impl MultiplexEventSink {
    fn new(emit_stdout: bool, broadcast_tx: broadcast::Sender<AgentEvent>) -> Self {
        Self {
            emit_stdout,
            broadcast_tx,
        }
    }
}

#[async_trait]
impl EventSink for MultiplexEventSink {
    async fn emit(&self, event: AgentEvent) -> KelvinResult<()> {
        let _ = self.broadcast_tx.send(event.clone());
        if self.emit_stdout {
            println!("{}", serde_json::to_string(&event).unwrap_or_default());
        }
        Ok(())
    }
}

#[derive(Default)]
struct HashMapToolRegistry {
    tools: std::sync::RwLock<HashMap<String, Arc<dyn Tool>>>,
}

impl HashMapToolRegistry {
    fn register<T>(&self, tool: T)
    where
        T: Tool + 'static,
    {
        let name = tool.name().to_string();
        self.tools
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name, Arc::new(tool));
    }
}

impl ToolRegistry for HashMapToolRegistry {
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .cloned()
    }

    fn names(&self) -> Vec<String> {
        self.tools
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .cloned()
            .collect()
    }
}

#[derive(Clone)]
struct CombinedToolRegistry {
    registries: Vec<Arc<dyn ToolRegistry>>,
}

impl CombinedToolRegistry {
    fn new(registries: Vec<Arc<dyn ToolRegistry>>) -> Self {
        Self { registries }
    }
}

impl ToolRegistry for CombinedToolRegistry {
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        for registry in &self.registries {
            if let Some(tool) = registry.get(name) {
                return Some(tool);
            }
        }
        None
    }

    fn names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for registry in &self.registries {
            names.extend(registry.names());
        }
        names.sort();
        names.dedup();
        names
    }
}

type InstalledModelProviders = Vec<(String, Arc<dyn ModelProvider>)>;
type LoadedInstalledPlugins = (Arc<dyn ToolRegistry>, InstalledModelProviders, usize);
type SessionDescriptorMap = HashMap<String, SessionDescriptor>;
type SessionMessagesMap = HashMap<String, Vec<SessionMessage>>;

#[derive(Debug, Clone)]
struct TimeTool;

#[async_trait]
impl Tool for TimeTool {
    fn name(&self) -> &str {
        "time"
    }

    async fn call(&self, _input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        Ok(ToolCallResult {
            summary: "timestamp generated".to_string(),
            output: Some(now_ms.to_string()),
            visible_text: Some(format!("Current unix epoch millis: {now_ms}")),
            is_error: false,
        })
    }
}

#[derive(Debug, Clone)]
struct StaticTextTool {
    name: String,
    text: String,
}

impl StaticTextTool {
    fn new(name: &str, text: &str) -> Self {
        Self {
            name: name.to_string(),
            text: text.to_string(),
        }
    }
}

#[async_trait]
impl Tool for StaticTextTool {
    fn name(&self) -> &str {
        &self.name
    }

    async fn call(&self, _input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        Ok(ToolCallResult {
            summary: format!("{} returned static text", self.name),
            output: Some(self.text.clone()),
            visible_text: Some(self.text.clone()),
            is_error: false,
        })
    }
}

#[derive(Clone)]
struct FailoverModelProvider {
    providers: Vec<Arc<dyn ModelProvider>>,
    chain_label: String,
    max_retries_per_provider: u8,
    retry_backoff_ms: u64,
}

impl FailoverModelProvider {
    fn is_retryable(err: &KelvinError) -> bool {
        matches!(
            err,
            KelvinError::Backend(_) | KelvinError::Timeout(_) | KelvinError::Io(_)
        )
    }
}

#[async_trait]
impl ModelProvider for FailoverModelProvider {
    fn provider_name(&self) -> &str {
        "kelvin.failover"
    }

    fn model_name(&self) -> &str {
        &self.chain_label
    }

    async fn infer(&self, input: ModelInput) -> KelvinResult<ModelOutput> {
        let mut failure_chain = Vec::new();

        for provider in &self.providers {
            let provider_label = format!("{}/{}", provider.provider_name(), provider.model_name());
            for attempt in 0..=self.max_retries_per_provider {
                match provider.infer(input.clone()).await {
                    Ok(output) => return Ok(output),
                    Err(err) => {
                        if !Self::is_retryable(&err) {
                            return Err(err);
                        }
                        let attempt_number = attempt.saturating_add(1);
                        failure_chain
                            .push(format!("{provider_label} attempt {attempt_number}: {err}"));
                        if attempt < self.max_retries_per_provider {
                            sleep(Duration::from_millis(self.retry_backoff_ms.max(1))).await;
                        }
                    }
                }
            }
        }

        Err(KelvinError::Backend(format!(
            "all model providers failed: {}",
            failure_chain.join(" | ")
        )))
    }
}

#[derive(Clone)]
pub struct KelvinSdkRuntime {
    runtime: CoreRuntime,
    default_workspace_dir: PathBuf,
    default_session_id: String,
    default_timeout_ms: u64,
    default_system_prompt: Option<String>,
    cli_plugin_tool: Option<Arc<dyn Tool>>,
    loaded_installed_plugins: usize,
    event_tx: broadcast::Sender<AgentEvent>,
    persistence: RuntimePersistence,
    scheduler_store: Arc<SchedulerStore>,
}

impl KelvinSdkRuntime {
    pub async fn initialize(config: KelvinSdkRuntimeConfig) -> KelvinResult<Self> {
        config.validate()?;
        let persistence = RuntimePersistence::new(
            config.state_dir.clone(),
            config.persist_runs,
            config.max_session_history_messages,
            config.compact_to_messages,
        )?;
        let scheduler_store = Arc::new(SchedulerStore::new(
            config.state_dir.clone(),
            &config.workspace_dir,
        )?);
        let session_store = Arc::new(FileBackedSessionStore::load(persistence.clone())?);
        let (event_tx, _) = broadcast::channel(1_024);
        let event_sink: Arc<dyn EventSink> = if config.emit_stdout_events {
            Arc::new(StdoutEventSink)
        } else {
            Arc::new(MultiplexEventSink::new(false, event_tx.clone()))
        };

        let builtin_tools = Arc::new(HashMapToolRegistry::default());
        builtin_tools.register(TimeTool);
        builtin_tools.register(StaticTextTool::new(
            "hello_tool",
            "Hello from Kelvin SDK built-in tools.",
        ));
        let (toolpack_tools, toolpack_count) =
            toolpack::load_default_toolpack_plugins(&config.core_version, scheduler_store.clone())?;
        println!("loaded kelvin core toolpack plugins: {toolpack_count}");

        let (installed_tools, installed_models, loaded_installed_plugins): LoadedInstalledPlugins =
            if config.load_installed_plugins {
                let loaded = load_installed_plugins_default(
                    &config.core_version,
                    config.plugin_security_policy.clone(),
                )?;
                println!("loaded installed plugins: {}", loaded.loaded_plugins.len());

                let mut models = Vec::new();
                for plugin_id in loaded.model_registry.plugin_ids() {
                    if let Some(provider) = loaded.model_registry.get_by_plugin_id(&plugin_id) {
                        models.push((plugin_id, provider));
                    }
                }
                (loaded.tool_registry, models, loaded.loaded_plugins.len())
            } else {
                (Arc::new(HashMapToolRegistry::default()), Vec::new(), 0)
            };

        let cli_plugin_tool = if config.require_cli_plugin_tool {
            Some(installed_tools.get("kelvin_cli").ok_or_else(|| {
                KelvinError::NotFound(
                    "required plugin tool 'kelvin_cli' not found; install it with scripts/install-kelvin-cli-plugin.sh"
                        .to_string(),
                )
            })?)
        } else {
            installed_tools.get("kelvin_cli")
        };

        let tools: Arc<dyn ToolRegistry> = Arc::new(CombinedToolRegistry::new(vec![
            installed_tools,
            toolpack_tools,
            builtin_tools,
        ]));

        #[cfg(feature = "memory_rpc")]
        let memory: Arc<dyn MemorySearchManager> = {
            let mut rpc_cfg = MemoryClientConfig::from_env();
            rpc_cfg.workspace_id = config.workspace_dir.to_string_lossy().to_string();
            rpc_cfg.session_id = config.default_session_id.clone();
            match RpcMemoryManager::connect(rpc_cfg).await {
                Ok(manager) => {
                    println!("using rpc memory manager");
                    Arc::new(manager)
                }
                Err(err) => {
                    #[cfg(feature = "memory_legacy_fallback")]
                    {
                        eprintln!(
                            "warning: rpc memory unavailable, falling back to legacy in-proc memory: {err}"
                        );
                        MemoryFactory::build(
                            &config.workspace_dir,
                            config.memory_mode.as_backend_kind(),
                        )
                    }
                    #[cfg(not(feature = "memory_legacy_fallback"))]
                    {
                        return Err(KelvinError::Backend(format!(
                            "memory controller unavailable and legacy fallback disabled: {err}"
                        )));
                    }
                }
            }
        };

        #[cfg(not(feature = "memory_rpc"))]
        let memory: Arc<dyn MemorySearchManager> =
            MemoryFactory::build(&config.workspace_dir, config.memory_mode.as_backend_kind());

        let model = resolve_model_provider(
            &config.model_provider,
            &installed_models,
            config.load_installed_plugins,
        )?;
        let brain = Arc::new(KelvinBrain::new(
            session_store,
            memory,
            model,
            tools,
            event_sink,
        ));
        let runtime = CoreRuntime::new(brain);
        Ok(Self {
            runtime,
            default_workspace_dir: config.workspace_dir,
            default_session_id: config.default_session_id,
            default_timeout_ms: config.default_timeout_ms,
            default_system_prompt: config.default_system_prompt,
            cli_plugin_tool,
            loaded_installed_plugins,
            event_tx,
            persistence,
            scheduler_store,
        })
    }

    pub fn loaded_installed_plugins(&self) -> usize {
        self.loaded_installed_plugins
    }

    pub fn state_dir(&self) -> Option<&Path> {
        self.persistence.state_dir.as_deref()
    }

    pub fn scheduler_store(&self) -> Arc<SchedulerStore> {
        self.scheduler_store.clone()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    pub async fn submit(&self, request: KelvinSdkRunRequest) -> KelvinResult<KelvinSdkAcceptedRun> {
        let prompt = request.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(KelvinError::InvalidInput(
                "prompt must not be empty".to_string(),
            ));
        }
        let run_id = request
            .run_id
            .unwrap_or_else(|| format!("run-{}", Uuid::new_v4()));
        let session_id = request
            .session_id
            .unwrap_or_else(|| self.default_session_id.clone());
        let workspace_dir = request
            .workspace_dir
            .unwrap_or_else(|| self.default_workspace_dir.clone());
        let timeout_ms = request.timeout_ms.unwrap_or(self.default_timeout_ms);
        let system_prompt = request
            .system_prompt
            .or_else(|| self.default_system_prompt.clone());

        let cli_plugin_preflight = if let Some(cli_plugin_tool) = &self.cli_plugin_tool {
            Some(
                cli_plugin_tool
                    .call(ToolCallInput {
                        run_id: run_id.clone(),
                        session_id: session_id.clone(),
                        workspace_dir: workspace_dir.to_string_lossy().to_string(),
                        arguments: json!({"prompt": prompt}),
                    })
                    .await?
                    .summary,
            )
        } else {
            None
        };

        let prompt_length = prompt.len();
        let session_id_for_record = session_id.clone();
        let workspace_dir_for_record = workspace_dir.clone();
        let accepted = self
            .runtime
            .submit(AgentRunRequest {
                run_id: run_id.clone(),
                session_id: session_id.clone(),
                session_key: session_id,
                workspace_dir: workspace_dir.to_string_lossy().to_string(),
                prompt,
                extra_system_prompt: system_prompt,
                timeout_ms: Some(timeout_ms),
                memory_query: request.memory_query,
            })
            .await?;

        self.persistence.persist_run_record(
            &accepted.run_id,
            json!({
                "accepted_at_ms": accepted.accepted_at_ms,
                "session_id": session_id_for_record,
                "workspace_dir": workspace_dir_for_record.to_string_lossy().to_string(),
                "prompt_length": prompt_length,
            }),
        )?;

        Ok(KelvinSdkAcceptedRun {
            run_id: accepted.run_id,
            accepted_at_ms: accepted.accepted_at_ms,
            cli_plugin_preflight,
        })
    }

    pub async fn state(&self, run_id: &str) -> KelvinResult<RunState> {
        let state = self.runtime.state(run_id).await?;
        self.persistence
            .persist_run_record(run_id, json!({ "last_state": state }))?;
        Ok(state)
    }

    pub async fn wait(&self, run_id: &str, timeout_ms: u64) -> KelvinResult<AgentWaitResult> {
        let wait = self.runtime.wait(run_id, timeout_ms).await?;
        self.persistence
            .persist_run_record(run_id, json!({ "last_wait": wait }))?;
        Ok(wait)
    }

    pub async fn wait_for_outcome(
        &self,
        run_id: &str,
        timeout_ms: u64,
    ) -> KelvinResult<RunOutcome> {
        let outcome = self.runtime.wait_for_outcome(run_id, timeout_ms).await?;
        let persisted_outcome = match &outcome {
            RunOutcome::Completed(result) => json!({
                "status": "completed",
                "result": result,
            }),
            RunOutcome::Failed(error) => json!({
                "status": "failed",
                "error": error,
            }),
            RunOutcome::Timeout => json!({
                "status": "timeout",
            }),
        };
        self.persistence
            .persist_run_record(run_id, json!({ "last_outcome": persisted_outcome }))?;
        Ok(outcome)
    }
}

fn resolve_model_provider(
    selection: &KelvinSdkModelSelection,
    installed_models: &InstalledModelProviders,
    load_installed_plugins: bool,
) -> KelvinResult<Arc<dyn ModelProvider>> {
    let resolve_installed = |plugin_id: &str| -> KelvinResult<Arc<dyn ModelProvider>> {
        if !load_installed_plugins {
            return Err(KelvinError::InvalidInput(format!(
                "model provider '{}' requested but load_installed_plugins is false",
                plugin_id
            )));
        }
        installed_models
            .iter()
            .find_map(|(id, provider)| {
                if id == plugin_id {
                    Some(provider.clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                KelvinError::NotFound(format!(
                    "configured model provider plugin '{}' not found; install it and ensure policy allows it",
                    plugin_id
                ))
            })
    };

    match selection {
        KelvinSdkModelSelection::Echo => Ok(Arc::new(EchoModelProvider::new("kelvin", "echo-v1"))),
        KelvinSdkModelSelection::InstalledPlugin { plugin_id } => resolve_installed(plugin_id),
        KelvinSdkModelSelection::InstalledPluginFailover {
            plugin_ids,
            max_retries_per_provider,
            retry_backoff_ms,
        } => {
            if plugin_ids.is_empty() {
                return Err(KelvinError::InvalidInput(
                    "model failover requires at least one plugin id".to_string(),
                ));
            }

            let mut providers = Vec::with_capacity(plugin_ids.len());
            let mut chain_names = Vec::with_capacity(plugin_ids.len());
            for plugin_id in plugin_ids {
                let provider = resolve_installed(plugin_id)?;
                chain_names.push(format!(
                    "{}/{}",
                    provider.provider_name(),
                    provider.model_name()
                ));
                providers.push(provider);
            }

            Ok(Arc::new(FailoverModelProvider {
                providers,
                chain_label: chain_names.join(" -> "),
                max_retries_per_provider: *max_retries_per_provider,
                retry_backoff_ms: (*retry_backoff_ms).max(1),
            }))
        }
    }
}

pub async fn run_with_sdk(config: KelvinSdkConfig) -> KelvinResult<KelvinRunSummary> {
    config.validate()?;
    let runtime =
        KelvinSdkRuntime::initialize(KelvinSdkRuntimeConfig::from_run_config(&config)).await?;
    let accepted = runtime
        .submit(KelvinSdkRunRequest {
            prompt: config.prompt,
            session_id: Some(config.session_id),
            workspace_dir: Some(config.workspace_dir),
            timeout_ms: Some(config.timeout_ms),
            system_prompt: config.system_prompt,
            memory_query: None,
            run_id: Some(format!("run-{}", now_ms())),
        })
        .await?;
    println!(
        "accepted run: {} at {}",
        accepted.run_id, accepted.accepted_at_ms
    );

    match runtime
        .wait_for_outcome(&accepted.run_id, config.timeout_ms.saturating_add(5_000))
        .await?
    {
        RunOutcome::Completed(result) => Ok(KelvinRunSummary {
            run_id: accepted.run_id,
            accepted_at_ms: accepted.accepted_at_ms,
            provider: result.meta.provider,
            model: result.meta.model,
            duration_ms: result.meta.duration_ms,
            payloads: result.payloads.into_iter().map(|item| item.text).collect(),
            loaded_installed_plugins: runtime.loaded_installed_plugins(),
            cli_plugin_preflight: accepted
                .cli_plugin_preflight
                .unwrap_or_else(|| "not required".to_string()),
        }),
        RunOutcome::Failed(error) => Err(KelvinError::Backend(format!("run failed: {error}"))),
        RunOutcome::Timeout => Err(KelvinError::Timeout(
            "timed out waiting for run result".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use async_trait::async_trait;
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use kelvin_core::{
        KelvinError, ModelInput, ModelOutput, ModelProvider, RunOutcome,
        ANTHROPIC_MESSAGES_PROFILE_ID, OPENAI_RESPONSES_PROFILE_ID,
    };
    use mockito::Server;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::{run_with_sdk, KelvinCliMemoryMode, KelvinSdkConfig, KelvinSdkModelSelection};

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const TEST_PUBLISHER_ID: &str = "kelvin_sdk_test";
    const TEST_SIGNING_KEY_BYTES: [u8; 32] = [31_u8; 32];

    #[derive(Clone)]
    struct StubModelProvider {
        provider: String,
        model: String,
        remaining_failures: Arc<AtomicUsize>,
        calls: Arc<AtomicUsize>,
        failure: KelvinError,
        response_text: String,
    }

    impl StubModelProvider {
        fn new(
            provider: &str,
            model: &str,
            failures: usize,
            failure: KelvinError,
            response_text: &str,
        ) -> Self {
            Self {
                provider: provider.to_string(),
                model: model.to_string(),
                remaining_failures: Arc::new(AtomicUsize::new(failures)),
                calls: Arc::new(AtomicUsize::new(0)),
                failure,
                response_text: response_text.to_string(),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ModelProvider for StubModelProvider {
        fn provider_name(&self) -> &str {
            &self.provider
        }

        fn model_name(&self) -> &str {
            &self.model
        }

        async fn infer(&self, _input: ModelInput) -> kelvin_core::KelvinResult<ModelOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self
                .remaining_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                    if value == 0 {
                        None
                    } else {
                        Some(value.saturating_sub(1))
                    }
                })
                .is_ok()
            {
                return Err(self.failure.clone());
            }

            Ok(ModelOutput {
                assistant_text: self.response_text.clone(),
                stop_reason: Some("completed".to_string()),
                tool_calls: Vec::new(),
                usage: None,
            })
        }
    }

    fn unique_workspace() -> std::path::PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!("kelvin-sdk-test-{millis}"));
        std::fs::create_dir_all(&path).expect("create workspace");
        path
    }

    #[test]
    fn runtime_config_validate_rejects_duplicate_failover_plugin_ids() {
        let workspace = unique_workspace();
        let cfg = super::KelvinSdkRuntimeConfig {
            workspace_dir: workspace,
            default_session_id: "main".to_string(),
            memory_mode: KelvinCliMemoryMode::Markdown,
            default_timeout_ms: 1_000,
            default_system_prompt: None,
            core_version: "0.1.0".to_string(),
            plugin_security_policy: Default::default(),
            load_installed_plugins: true,
            model_provider: KelvinSdkModelSelection::InstalledPluginFailover {
                plugin_ids: vec!["dup.plugin".to_string(), "dup.plugin".to_string()],
                max_retries_per_provider: 1,
                retry_backoff_ms: 100,
            },
            require_cli_plugin_tool: false,
            emit_stdout_events: false,
            state_dir: None,
            persist_runs: false,
            max_session_history_messages: 128,
            compact_to_messages: 64,
        };
        let err = cfg.validate().expect_err("duplicate ids should fail");
        assert!(err
            .to_string()
            .contains("duplicate model provider plugin id"));
    }

    #[test]
    fn sdk_config_validate_rejects_missing_workspace() {
        let mut cfg = KelvinSdkConfig::for_prompt("hello");
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default();
        let missing = std::env::temp_dir().join(format!("kelvin-sdk-test-missing-{millis}"));
        if missing.exists() {
            std::fs::remove_dir_all(&missing).expect("cleanup stale path");
        }
        cfg.workspace_dir = missing;
        let err = cfg.validate().expect_err("missing workspace should fail");
        assert!(err.to_string().contains("workspace_dir does not exist"));
    }

    #[test]
    fn sdk_config_validate_rejects_non_semver_core_version() {
        let mut cfg = KelvinSdkConfig::for_prompt("hello");
        cfg.workspace_dir = unique_workspace();
        cfg.core_version = "not-semver".to_string();
        let err = cfg
            .validate()
            .expect_err("non-semver core version should fail");
        assert!(err
            .to_string()
            .contains("core_version must be valid semver"));
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut out = String::with_capacity(digest.len() * 2);
        for byte in digest {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    fn parse_wat(input: &str) -> Vec<u8> {
        wat::parse_str(input).expect("parse wat")
    }

    fn cli_test_wasm() -> Vec<u8> {
        parse_wat(
            r#"
            (module
              (import "claw" "send_message" (func $send_message (param i32) (result i32)))
              (func (export "run") (result i32)
                i32.const 7
                call $send_message
                drop
                i32.const 0)
            )
            "#,
        )
    }

    fn model_test_wasm(import_name: &str) -> Vec<u8> {
        parse_wat(&format!(
            r#"
            (module
              (import "kelvin_model_host_v1" "{import_name}" (func $provider_call (param i32 i32) (result i64)))
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
                call $provider_call)
            )
            "#,
        ))
    }

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&TEST_SIGNING_KEY_BYTES)
    }

    fn write_trust_policy_file(trust_policy_path: &Path, signing_key: &SigningKey) {
        if let Some(parent) = trust_policy_path.parent() {
            std::fs::create_dir_all(parent).expect("create trust policy parent");
        }
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());
        let trust_policy = json!({
            "require_signature": true,
            "publishers": [
                {
                    "id": TEST_PUBLISHER_ID,
                    "ed25519_public_key": public_key,
                }
            ]
        });
        std::fs::write(
            trust_policy_path,
            serde_json::to_vec_pretty(&trust_policy).expect("serialize trust policy"),
        )
        .expect("write trust policy file");
    }

    fn write_signed_plugin(
        plugin_home: &Path,
        plugin_id: &str,
        version: &str,
        entrypoint: &str,
        wasm_bytes: &[u8],
        mut manifest: serde_json::Value,
        signing_key: &SigningKey,
    ) {
        let version_dir = plugin_home.join(plugin_id).join(version);
        let payload_dir = version_dir.join("payload");
        std::fs::create_dir_all(&payload_dir).expect("create payload dir");

        std::fs::write(payload_dir.join(entrypoint), wasm_bytes).expect("write wasm payload");
        manifest["entrypoint_sha256"] = json!(sha256_hex(wasm_bytes));
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest bytes");
        std::fs::write(version_dir.join("plugin.json"), &manifest_bytes).expect("write manifest");

        let signature = signing_key.sign(&manifest_bytes);
        let signature_base64 =
            base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
        std::fs::write(version_dir.join("plugin.sig"), signature_base64).expect("write signature");
    }

    fn seed_cli_plugin_for_tests(plugin_home: &Path, trust_policy_path: &Path) {
        let signing_key = test_signing_key();
        write_signed_plugin(
            plugin_home,
            "kelvin.cli",
            "0.1.0",
            "kelvin_cli.wasm",
            &cli_test_wasm(),
            json!({
                "id": "kelvin.cli",
                "name": "Kelvin CLI Plugin (Test)",
                "version": "0.1.0",
                "api_version": "1.0.0",
                "description": "test cli plugin",
                "homepage": "https://example.com/kelvin-cli-test-plugin",
                "capabilities": ["tool_provider"],
                "experimental": false,
                "min_core_version": "0.1.0",
                "max_core_version": null,
                "runtime": "wasm_tool_v1",
                "tool_name": "kelvin_cli",
                "entrypoint": "kelvin_cli.wasm",
                "entrypoint_sha256": null,
                "publisher": TEST_PUBLISHER_ID,
                "capability_scopes": {
                    "fs_read_paths": [],
                    "network_allow_hosts": []
                },
                "operational_controls": {
                    "timeout_ms": 2000,
                    "max_retries": 0,
                    "max_calls_per_minute": 120,
                    "circuit_breaker_failures": 3,
                    "circuit_breaker_cooldown_ms": 30000
                }
            }),
            &signing_key,
        );
        write_trust_policy_file(trust_policy_path, &signing_key);
    }

    #[allow(clippy::too_many_arguments)]
    fn seed_model_plugin_for_mock_host(
        plugin_home: &Path,
        trust_policy_path: &Path,
        plugin_id: &str,
        plugin_name: &str,
        provider_name: &str,
        provider_profile: &str,
        model_name: &str,
        allow_host: &str,
        entrypoint: &str,
    ) {
        let signing_key = test_signing_key();
        let wasm_bytes = model_test_wasm("provider_profile_call");
        write_signed_plugin(
            plugin_home,
            plugin_id,
            "0.1.0",
            entrypoint,
            &wasm_bytes,
            json!({
                "id": plugin_id,
                "name": plugin_name,
                "version": "0.1.0",
                "api_version": "1.0.0",
                "description": format!("test {provider_name} model plugin"),
                "homepage": format!("https://example.com/{provider_name}-plugin"),
                "capabilities": ["model_provider"],
                "experimental": false,
                "min_core_version": "0.1.0",
                "max_core_version": null,
                "runtime": "wasm_model_v1",
                "provider_name": provider_name,
                "provider_profile": provider_profile,
                "model_name": model_name,
                "entrypoint": entrypoint,
                "entrypoint_sha256": null,
                "publisher": TEST_PUBLISHER_ID,
                "capability_scopes": {
                    "fs_read_paths": [],
                    "network_allow_hosts": [allow_host]
                },
                "operational_controls": {
                    "timeout_ms": 5000,
                    "max_retries": 0,
                    "max_calls_per_minute": 120,
                    "circuit_breaker_failures": 3,
                    "circuit_breaker_cooldown_ms": 1000
                }
            }),
            &signing_key,
        );
        write_trust_policy_file(trust_policy_path, &signing_key);
    }

    fn seed_openai_plugin_for_mock_host(
        plugin_home: &Path,
        trust_policy_path: &Path,
        allow_host: &str,
    ) {
        seed_model_plugin_for_mock_host(
            plugin_home,
            trust_policy_path,
            "acme.openai",
            "Acme OpenAI Plugin",
            "openai",
            OPENAI_RESPONSES_PROFILE_ID,
            "gpt-4.1-mini",
            allow_host,
            "kelvin_openai.wasm",
        );
    }

    fn seed_anthropic_plugin_for_mock_host(
        plugin_home: &Path,
        trust_policy_path: &Path,
        allow_host: &str,
    ) {
        seed_model_plugin_for_mock_host(
            plugin_home,
            trust_policy_path,
            "acme.anthropic",
            "Acme Anthropic Plugin",
            "anthropic",
            ANTHROPIC_MESSAGES_PROFILE_ID,
            "claude-3-5-haiku-latest",
            allow_host,
            "kelvin_anthropic.wasm",
        );
    }

    #[tokio::test]
    async fn run_with_sdk_executes_cli_plugin_and_returns_payload() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fixture_root = unique_workspace();
        let plugin_home = fixture_root.join("plugins");
        let trust_policy = fixture_root.join("trusted_publishers.json");
        seed_cli_plugin_for_tests(&plugin_home, &trust_policy);
        assert!(
            trust_policy.is_file(),
            "missing trust policy file: {}",
            trust_policy.to_string_lossy()
        );

        let previous_plugin_home = std::env::var("KELVIN_PLUGIN_HOME").ok();
        let previous_trust_policy = std::env::var("KELVIN_TRUST_POLICY_PATH").ok();
        std::env::set_var("KELVIN_PLUGIN_HOME", plugin_home.as_os_str());
        std::env::set_var("KELVIN_TRUST_POLICY_PATH", trust_policy.as_os_str());

        let workspace = unique_workspace();
        let mut config = KelvinSdkConfig::for_prompt("hello sdk");
        config.workspace_dir = workspace;
        config.timeout_ms = 5_000;
        config.memory_mode = KelvinCliMemoryMode::Fallback;
        config.load_installed_plugins = true;

        let result = run_with_sdk(config).await.expect("run with sdk");

        match previous_plugin_home {
            Some(value) => std::env::set_var("KELVIN_PLUGIN_HOME", value),
            None => std::env::remove_var("KELVIN_PLUGIN_HOME"),
        }
        match previous_trust_policy {
            Some(value) => std::env::set_var("KELVIN_TRUST_POLICY_PATH", value),
            None => std::env::remove_var("KELVIN_TRUST_POLICY_PATH"),
        }

        assert!(result.cli_plugin_preflight.contains("kelvin_cli executed"));
        assert!(result
            .payloads
            .iter()
            .any(|payload| payload.contains("Echo: hello sdk")));
    }

    #[tokio::test]
    async fn run_with_sdk_uses_installed_openai_model_plugin_via_mock_server() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let workspace = unique_workspace();
        let plugin_home = workspace.join("plugins");
        let trust_policy = workspace.join("trusted_publishers.json");
        seed_cli_plugin_for_tests(&plugin_home, &trust_policy);

        let mut server = Server::new_async().await;
        let response_body = json!({
            "assistant_text": "mock-openai-ok",
            "stop_reason": "completed",
            "tool_calls": [],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 4,
                "total_tokens": 14
            }
        })
        .to_string();
        let mock = server
            .mock("POST", "/v1/responses")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(response_body)
            .create_async()
            .await;
        let base_url = server.url();
        let allow_host = "127.0.0.1".to_string();
        seed_openai_plugin_for_mock_host(&plugin_home, &trust_policy, &allow_host);

        let previous_plugin_home = std::env::var("KELVIN_PLUGIN_HOME").ok();
        let previous_trust_policy = std::env::var("KELVIN_TRUST_POLICY_PATH").ok();
        let previous_openai_key = std::env::var("OPENAI_API_KEY").ok();
        let previous_openai_base = std::env::var("OPENAI_BASE_URL").ok();
        std::env::set_var("KELVIN_PLUGIN_HOME", plugin_home.as_os_str());
        std::env::set_var("KELVIN_TRUST_POLICY_PATH", trust_policy.as_os_str());
        std::env::set_var("OPENAI_API_KEY", "test-key");
        std::env::set_var("OPENAI_BASE_URL", &base_url);

        let mut config = KelvinSdkConfig::for_prompt("hello openai");
        config.workspace_dir = workspace.clone();
        config.timeout_ms = 5_000;
        config.memory_mode = KelvinCliMemoryMode::Fallback;
        config.load_installed_plugins = true;
        config.model_provider = KelvinSdkModelSelection::InstalledPlugin {
            plugin_id: "acme.openai".to_string(),
        };

        let result = run_with_sdk(config).await.expect("run with openai plugin");
        mock.assert_async().await;

        match previous_plugin_home {
            Some(value) => std::env::set_var("KELVIN_PLUGIN_HOME", value),
            None => std::env::remove_var("KELVIN_PLUGIN_HOME"),
        }
        match previous_trust_policy {
            Some(value) => std::env::set_var("KELVIN_TRUST_POLICY_PATH", value),
            None => std::env::remove_var("KELVIN_TRUST_POLICY_PATH"),
        }
        match previous_openai_key {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
        match previous_openai_base {
            Some(value) => std::env::set_var("OPENAI_BASE_URL", value),
            None => std::env::remove_var("OPENAI_BASE_URL"),
        }

        assert_eq!(result.provider, "openai");
        assert_eq!(result.model, "gpt-4.1-mini");
        assert!(result
            .payloads
            .iter()
            .any(|payload| payload.contains("mock-openai-ok")));
    }

    #[tokio::test]
    async fn run_with_sdk_uses_installed_anthropic_model_plugin_via_mock_server() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let workspace = unique_workspace();
        let plugin_home = workspace.join("plugins");
        let trust_policy = workspace.join("trusted_publishers.json");
        seed_cli_plugin_for_tests(&plugin_home, &trust_policy);

        let mut server = Server::new_async().await;
        let response_body = json!({
            "assistant_text": "mock-anthropic-ok",
            "stop_reason": "completed",
            "tool_calls": [],
            "usage": {
                "input_tokens": 12,
                "output_tokens": 5,
                "total_tokens": 17
            }
        })
        .to_string();
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(response_body)
            .create_async()
            .await;
        let base_url = server.url();
        let allow_host = "127.0.0.1".to_string();
        seed_anthropic_plugin_for_mock_host(&plugin_home, &trust_policy, &allow_host);

        let previous_plugin_home = std::env::var("KELVIN_PLUGIN_HOME").ok();
        let previous_trust_policy = std::env::var("KELVIN_TRUST_POLICY_PATH").ok();
        let previous_anthropic_key = std::env::var("ANTHROPIC_API_KEY").ok();
        let previous_anthropic_base = std::env::var("ANTHROPIC_BASE_URL").ok();
        std::env::set_var("KELVIN_PLUGIN_HOME", plugin_home.as_os_str());
        std::env::set_var("KELVIN_TRUST_POLICY_PATH", trust_policy.as_os_str());
        std::env::set_var("ANTHROPIC_API_KEY", "test-anthropic-key");
        std::env::set_var("ANTHROPIC_BASE_URL", &base_url);

        let mut config = KelvinSdkConfig::for_prompt("hello anthropic");
        config.workspace_dir = workspace.clone();
        config.timeout_ms = 5_000;
        config.memory_mode = KelvinCliMemoryMode::Fallback;
        config.load_installed_plugins = true;
        config.model_provider = KelvinSdkModelSelection::InstalledPlugin {
            plugin_id: "acme.anthropic".to_string(),
        };

        let result = run_with_sdk(config)
            .await
            .expect("run with anthropic plugin");
        mock.assert_async().await;

        match previous_plugin_home {
            Some(value) => std::env::set_var("KELVIN_PLUGIN_HOME", value),
            None => std::env::remove_var("KELVIN_PLUGIN_HOME"),
        }
        match previous_trust_policy {
            Some(value) => std::env::set_var("KELVIN_TRUST_POLICY_PATH", value),
            None => std::env::remove_var("KELVIN_TRUST_POLICY_PATH"),
        }
        match previous_anthropic_key {
            Some(value) => std::env::set_var("ANTHROPIC_API_KEY", value),
            None => std::env::remove_var("ANTHROPIC_API_KEY"),
        }
        match previous_anthropic_base {
            Some(value) => std::env::set_var("ANTHROPIC_BASE_URL", value),
            None => std::env::remove_var("ANTHROPIC_BASE_URL"),
        }

        assert_eq!(result.provider, "anthropic");
        assert_eq!(result.model, "claude-3-5-haiku-latest");
        assert!(result
            .payloads
            .iter()
            .any(|payload| payload.contains("mock-anthropic-ok")));
    }

    #[tokio::test]
    async fn run_with_sdk_installed_provider_failover_uses_second_profile() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let workspace = unique_workspace();
        let plugin_home = workspace.join("plugins");
        let trust_policy = workspace.join("trusted_publishers.json");
        seed_cli_plugin_for_tests(&plugin_home, &trust_policy);

        let mut openai_server = Server::new_async().await;
        let openai_mock = openai_server
            .mock("POST", "/v1/responses")
            .with_status(503)
            .with_body("upstream unavailable")
            .create_async()
            .await;
        let mut anthropic_server = Server::new_async().await;
        let anthropic_response = json!({
            "assistant_text": "mock-failover-anthropic-ok",
            "stop_reason": "completed",
            "tool_calls": [],
            "usage": {
                "input_tokens": 7,
                "output_tokens": 6,
                "total_tokens": 13
            }
        })
        .to_string();
        let anthropic_mock = anthropic_server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(anthropic_response)
            .create_async()
            .await;
        let allow_host = "127.0.0.1".to_string();
        seed_openai_plugin_for_mock_host(&plugin_home, &trust_policy, &allow_host);
        seed_anthropic_plugin_for_mock_host(&plugin_home, &trust_policy, &allow_host);

        let previous_plugin_home = std::env::var("KELVIN_PLUGIN_HOME").ok();
        let previous_trust_policy = std::env::var("KELVIN_TRUST_POLICY_PATH").ok();
        let previous_openai_key = std::env::var("OPENAI_API_KEY").ok();
        let previous_openai_base = std::env::var("OPENAI_BASE_URL").ok();
        let previous_anthropic_key = std::env::var("ANTHROPIC_API_KEY").ok();
        let previous_anthropic_base = std::env::var("ANTHROPIC_BASE_URL").ok();
        std::env::set_var("KELVIN_PLUGIN_HOME", plugin_home.as_os_str());
        std::env::set_var("KELVIN_TRUST_POLICY_PATH", trust_policy.as_os_str());
        std::env::set_var("OPENAI_API_KEY", "test-openai-key");
        std::env::set_var("OPENAI_BASE_URL", openai_server.url());
        std::env::set_var("ANTHROPIC_API_KEY", "test-anthropic-key");
        std::env::set_var("ANTHROPIC_BASE_URL", anthropic_server.url());

        let mut config = KelvinSdkConfig::for_prompt("hello failover");
        config.workspace_dir = workspace.clone();
        config.timeout_ms = 5_000;
        config.memory_mode = KelvinCliMemoryMode::Fallback;
        config.load_installed_plugins = true;
        config.model_provider = KelvinSdkModelSelection::InstalledPluginFailover {
            plugin_ids: vec!["acme.openai".to_string(), "acme.anthropic".to_string()],
            max_retries_per_provider: 0,
            retry_backoff_ms: 1,
        };

        let result = run_with_sdk(config).await.expect("run with model failover");
        openai_mock.assert_async().await;
        anthropic_mock.assert_async().await;

        match previous_plugin_home {
            Some(value) => std::env::set_var("KELVIN_PLUGIN_HOME", value),
            None => std::env::remove_var("KELVIN_PLUGIN_HOME"),
        }
        match previous_trust_policy {
            Some(value) => std::env::set_var("KELVIN_TRUST_POLICY_PATH", value),
            None => std::env::remove_var("KELVIN_TRUST_POLICY_PATH"),
        }
        match previous_openai_key {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
        match previous_openai_base {
            Some(value) => std::env::set_var("OPENAI_BASE_URL", value),
            None => std::env::remove_var("OPENAI_BASE_URL"),
        }
        match previous_anthropic_key {
            Some(value) => std::env::set_var("ANTHROPIC_API_KEY", value),
            None => std::env::remove_var("ANTHROPIC_API_KEY"),
        }
        match previous_anthropic_base {
            Some(value) => std::env::set_var("ANTHROPIC_BASE_URL", value),
            None => std::env::remove_var("ANTHROPIC_BASE_URL"),
        }

        assert_eq!(result.provider, "kelvin.failover");
        assert!(result.model.contains("openai/gpt-4.1-mini"));
        assert!(result.model.contains("anthropic/claude-3-5-haiku-latest"));
        assert!(result
            .payloads
            .iter()
            .any(|payload| payload.contains("mock-failover-anthropic-ok")));
    }

    #[tokio::test]
    async fn run_with_sdk_fails_when_configured_model_plugin_missing() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fixture_root = unique_workspace();
        let plugin_home = fixture_root.join("plugins");
        let trust_policy = fixture_root.join("trusted_publishers.json");
        seed_cli_plugin_for_tests(&plugin_home, &trust_policy);

        let previous_plugin_home = std::env::var("KELVIN_PLUGIN_HOME").ok();
        let previous_trust_policy = std::env::var("KELVIN_TRUST_POLICY_PATH").ok();
        std::env::set_var("KELVIN_PLUGIN_HOME", plugin_home.as_os_str());
        std::env::set_var("KELVIN_TRUST_POLICY_PATH", trust_policy.as_os_str());

        let workspace = unique_workspace();
        let mut config = KelvinSdkConfig::for_prompt("hello sdk");
        config.workspace_dir = workspace;
        config.timeout_ms = 5_000;
        config.memory_mode = KelvinCliMemoryMode::Fallback;
        config.load_installed_plugins = true;
        config.model_provider = KelvinSdkModelSelection::InstalledPlugin {
            plugin_id: "missing.model".to_string(),
        };

        let err = run_with_sdk(config)
            .await
            .expect_err("missing plugin should fail");

        match previous_plugin_home {
            Some(value) => std::env::set_var("KELVIN_PLUGIN_HOME", value),
            None => std::env::remove_var("KELVIN_PLUGIN_HOME"),
        }
        match previous_trust_policy {
            Some(value) => std::env::set_var("KELVIN_TRUST_POLICY_PATH", value),
            None => std::env::remove_var("KELVIN_TRUST_POLICY_PATH"),
        }

        assert!(err.to_string().contains("configured model provider plugin"));
    }

    #[tokio::test]
    async fn run_with_sdk_openai_plugin_requires_api_key() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let workspace = unique_workspace();
        let plugin_home = workspace.join("plugins");
        let trust_policy = workspace.join("trusted_publishers.json");
        seed_cli_plugin_for_tests(&plugin_home, &trust_policy);

        let base_url = "http://127.0.0.1:18080".to_string();
        let allow_host = "127.0.0.1".to_string();
        seed_openai_plugin_for_mock_host(&plugin_home, &trust_policy, &allow_host);

        let previous_plugin_home = std::env::var("KELVIN_PLUGIN_HOME").ok();
        let previous_trust_policy = std::env::var("KELVIN_TRUST_POLICY_PATH").ok();
        let previous_openai_key = std::env::var("OPENAI_API_KEY").ok();
        let previous_openai_base = std::env::var("OPENAI_BASE_URL").ok();
        std::env::set_var("KELVIN_PLUGIN_HOME", plugin_home.as_os_str());
        std::env::set_var("KELVIN_TRUST_POLICY_PATH", trust_policy.as_os_str());
        std::env::remove_var("OPENAI_API_KEY");
        std::env::set_var("OPENAI_BASE_URL", &base_url);

        let mut config = KelvinSdkConfig::for_prompt("hello openai");
        config.workspace_dir = workspace.clone();
        config.timeout_ms = 5_000;
        config.memory_mode = KelvinCliMemoryMode::Fallback;
        config.load_installed_plugins = true;
        config.model_provider = KelvinSdkModelSelection::InstalledPlugin {
            plugin_id: "acme.openai".to_string(),
        };

        let err = run_with_sdk(config)
            .await
            .expect_err("missing OPENAI_API_KEY should fail");

        match previous_plugin_home {
            Some(value) => std::env::set_var("KELVIN_PLUGIN_HOME", value),
            None => std::env::remove_var("KELVIN_PLUGIN_HOME"),
        }
        match previous_trust_policy {
            Some(value) => std::env::set_var("KELVIN_TRUST_POLICY_PATH", value),
            None => std::env::remove_var("KELVIN_TRUST_POLICY_PATH"),
        }
        match previous_openai_key {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
        match previous_openai_base {
            Some(value) => std::env::set_var("OPENAI_BASE_URL", value),
            None => std::env::remove_var("OPENAI_BASE_URL"),
        }

        assert!(err.to_string().contains("OPENAI_API_KEY"));
    }

    #[tokio::test]
    async fn failover_provider_retries_then_falls_back() {
        let primary = StubModelProvider::new(
            "primary",
            "model-a",
            2,
            KelvinError::Backend("primary unavailable".to_string()),
            "primary response",
        );
        let secondary = StubModelProvider::new(
            "secondary",
            "model-b",
            0,
            KelvinError::Backend("unused".to_string()),
            "secondary response",
        );
        let failover = super::FailoverModelProvider {
            providers: vec![Arc::new(primary.clone()), Arc::new(secondary.clone())],
            chain_label: "primary/model-a -> secondary/model-b".to_string(),
            max_retries_per_provider: 1,
            retry_backoff_ms: 1,
        };

        let output = failover
            .infer(ModelInput {
                run_id: "run-1".to_string(),
                session_id: "session-1".to_string(),
                system_prompt: "sys".to_string(),
                user_prompt: "hello".to_string(),
                memory_snippets: Vec::new(),
                history: Vec::new(),
            })
            .await
            .expect("failover output");
        assert_eq!(output.assistant_text, "secondary response");
        assert_eq!(primary.calls(), 2);
        assert_eq!(secondary.calls(), 1);
    }

    #[tokio::test]
    async fn failover_provider_does_not_fallback_on_invalid_input() {
        let primary = StubModelProvider::new(
            "primary",
            "model-a",
            1,
            KelvinError::InvalidInput("bad request".to_string()),
            "primary response",
        );
        let secondary = StubModelProvider::new(
            "secondary",
            "model-b",
            0,
            KelvinError::Backend("unused".to_string()),
            "secondary response",
        );
        let failover = super::FailoverModelProvider {
            providers: vec![Arc::new(primary.clone()), Arc::new(secondary.clone())],
            chain_label: "primary/model-a -> secondary/model-b".to_string(),
            max_retries_per_provider: 2,
            retry_backoff_ms: 1,
        };

        let err = failover
            .infer(ModelInput {
                run_id: "run-2".to_string(),
                session_id: "session-2".to_string(),
                system_prompt: "sys".to_string(),
                user_prompt: "hello".to_string(),
                memory_snippets: Vec::new(),
                history: Vec::new(),
            })
            .await
            .expect_err("invalid input should fail closed");
        assert!(matches!(err, KelvinError::InvalidInput(_)));
        assert_eq!(primary.calls(), 1);
        assert_eq!(secondary.calls(), 0);
    }

    fn find_first_messages_file(state_dir: &Path) -> Option<PathBuf> {
        let sessions_dir = state_dir.join("sessions");
        if !sessions_dir.is_dir() {
            return None;
        }
        let entries = fs::read_dir(sessions_dir).ok()?;
        for entry in entries {
            let entry = entry.ok()?;
            let path = entry.path().join("messages.jsonl");
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    fn find_run_record(state_dir: &Path, run_id: &str) -> Option<serde_json::Value> {
        let runs_dir = state_dir.join("runs");
        if !runs_dir.is_dir() {
            return None;
        }
        let entries = fs::read_dir(runs_dir).ok()?;
        for entry in entries {
            let entry = entry.ok()?;
            if !entry.path().is_file() {
                continue;
            }
            let bytes = fs::read(entry.path()).ok()?;
            let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            if value.get("run_id") == Some(&json!(run_id)) {
                return Some(value);
            }
        }
        None
    }

    #[tokio::test]
    async fn sdk_runtime_persists_run_records_and_outcomes() {
        let runtime_workspace = unique_workspace();
        let state_dir = runtime_workspace.join(".kelvin").join("state");
        let runtime = super::KelvinSdkRuntime::initialize(super::KelvinSdkRuntimeConfig {
            workspace_dir: runtime_workspace.clone(),
            default_session_id: "persist-session".to_string(),
            memory_mode: super::KelvinCliMemoryMode::Fallback,
            default_timeout_ms: 3_000,
            default_system_prompt: None,
            core_version: "0.1.0".to_string(),
            plugin_security_policy: Default::default(),
            load_installed_plugins: false,
            model_provider: super::KelvinSdkModelSelection::Echo,
            require_cli_plugin_tool: false,
            emit_stdout_events: false,
            state_dir: Some(state_dir.clone()),
            persist_runs: true,
            max_session_history_messages: 128,
            compact_to_messages: 64,
        })
        .await
        .expect("initialize runtime");

        let accepted = runtime
            .submit(super::KelvinSdkRunRequest {
                prompt: "persist me".to_string(),
                session_id: Some("persist-session".to_string()),
                workspace_dir: Some(runtime_workspace),
                timeout_ms: Some(3_000),
                system_prompt: None,
                memory_query: None,
                run_id: None,
            })
            .await
            .expect("submit");
        let outcome = runtime
            .wait_for_outcome(&accepted.run_id, 5_000)
            .await
            .expect("wait outcome");
        assert!(matches!(outcome, RunOutcome::Completed(_)));

        let run_record =
            find_run_record(&state_dir, &accepted.run_id).expect("run record should exist");
        assert_eq!(run_record["run_id"], json!(accepted.run_id));
        assert_eq!(run_record["last_outcome"]["status"], json!("completed"));
    }

    #[tokio::test]
    async fn sdk_runtime_compacts_session_history_by_policy() {
        let runtime_workspace = unique_workspace();
        let state_dir = runtime_workspace.join(".kelvin").join("state");
        let runtime = super::KelvinSdkRuntime::initialize(super::KelvinSdkRuntimeConfig {
            workspace_dir: runtime_workspace.clone(),
            default_session_id: "compact-session".to_string(),
            memory_mode: super::KelvinCliMemoryMode::Fallback,
            default_timeout_ms: 3_000,
            default_system_prompt: None,
            core_version: "0.1.0".to_string(),
            plugin_security_policy: Default::default(),
            load_installed_plugins: false,
            model_provider: super::KelvinSdkModelSelection::Echo,
            require_cli_plugin_tool: false,
            emit_stdout_events: false,
            state_dir: Some(state_dir.clone()),
            persist_runs: false,
            max_session_history_messages: 6,
            compact_to_messages: 3,
        })
        .await
        .expect("initialize runtime");

        for idx in 0..6 {
            let accepted = runtime
                .submit(super::KelvinSdkRunRequest {
                    prompt: format!("compaction message {idx}"),
                    session_id: Some("compact-session".to_string()),
                    workspace_dir: Some(runtime_workspace.clone()),
                    timeout_ms: Some(3_000),
                    system_prompt: None,
                    memory_query: None,
                    run_id: None,
                })
                .await
                .expect("submit");
            let _ = runtime
                .wait_for_outcome(&accepted.run_id, 5_000)
                .await
                .expect("wait outcome");
        }

        let messages_path = find_first_messages_file(&state_dir).expect("messages file");
        let content = fs::read_to_string(messages_path).expect("read messages");
        let lines = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        assert!(lines.len() <= 6);
    }

    #[tokio::test]
    async fn sdk_runtime_initialization_recovers_from_corrupt_descriptor() {
        let runtime_workspace = unique_workspace();
        let state_dir = runtime_workspace.join(".kelvin").join("state");
        let session_dir = state_dir
            .join("sessions")
            .join(super::RuntimePersistence::key_hex("recover-session"));
        fs::create_dir_all(&session_dir).expect("create session dir");
        fs::write(session_dir.join("descriptor.json"), b"{not-json").expect("write descriptor");

        let runtime = super::KelvinSdkRuntime::initialize(super::KelvinSdkRuntimeConfig {
            workspace_dir: runtime_workspace,
            default_session_id: "recover-session".to_string(),
            memory_mode: super::KelvinCliMemoryMode::Fallback,
            default_timeout_ms: 3_000,
            default_system_prompt: None,
            core_version: "0.1.0".to_string(),
            plugin_security_policy: Default::default(),
            load_installed_plugins: false,
            model_provider: super::KelvinSdkModelSelection::Echo,
            require_cli_plugin_tool: false,
            emit_stdout_events: false,
            state_dir: Some(state_dir.clone()),
            persist_runs: false,
            max_session_history_messages: 32,
            compact_to_messages: 16,
        })
        .await
        .expect("runtime should recover from corrupt descriptor");

        assert_eq!(runtime.loaded_installed_plugins(), 0);
        let quarantine_exists = fs::read_dir(&session_dir)
            .expect("read session dir")
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("descriptor.json.corrupt.")
            });
        assert!(
            quarantine_exists,
            "corrupt descriptor should be quarantined"
        );
    }

    #[tokio::test]
    async fn sdk_runtime_initialization_recovers_from_corrupt_messages_file() {
        let runtime_workspace = unique_workspace();
        let state_dir = runtime_workspace.join(".kelvin").join("state");
        let session_id = "recover-messages-session";
        let session_dir = state_dir
            .join("sessions")
            .join(super::RuntimePersistence::key_hex(session_id));
        fs::create_dir_all(&session_dir).expect("create session dir");
        let descriptor = json!({
            "session_id": session_id,
            "session_key": session_id,
            "workspace_dir": runtime_workspace.to_string_lossy(),
        });
        fs::write(
            session_dir.join("descriptor.json"),
            serde_json::to_vec(&descriptor).expect("descriptor bytes"),
        )
        .expect("write descriptor");
        fs::write(session_dir.join("messages.jsonl"), b"{bad-line").expect("write messages");

        let runtime = super::KelvinSdkRuntime::initialize(super::KelvinSdkRuntimeConfig {
            workspace_dir: runtime_workspace,
            default_session_id: session_id.to_string(),
            memory_mode: super::KelvinCliMemoryMode::Fallback,
            default_timeout_ms: 3_000,
            default_system_prompt: None,
            core_version: "0.1.0".to_string(),
            plugin_security_policy: Default::default(),
            load_installed_plugins: false,
            model_provider: super::KelvinSdkModelSelection::Echo,
            require_cli_plugin_tool: false,
            emit_stdout_events: false,
            state_dir: Some(state_dir.clone()),
            persist_runs: false,
            max_session_history_messages: 32,
            compact_to_messages: 16,
        })
        .await
        .expect("runtime should recover from corrupt messages");

        let accepted = runtime
            .submit(super::KelvinSdkRunRequest {
                prompt: "recovered".to_string(),
                session_id: Some(session_id.to_string()),
                workspace_dir: None,
                timeout_ms: Some(3_000),
                system_prompt: None,
                memory_query: None,
                run_id: None,
            })
            .await
            .expect("submit after recovery");
        let _ = runtime
            .wait_for_outcome(&accepted.run_id, 5_000)
            .await
            .expect("outcome");

        let quarantine_exists = fs::read_dir(&session_dir)
            .expect("read session dir")
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("messages.jsonl.corrupt.")
            });
        assert!(quarantine_exists, "corrupt messages should be quarantined");
    }
}
