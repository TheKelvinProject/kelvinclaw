use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::RwLock;

use kelvin_brain::{load_installed_plugins_default, EchoModelProvider, KelvinBrain};
use kelvin_core::{
    now_ms, AgentEvent, AgentRunRequest, CoreRuntime, EventSink, KelvinError, KelvinResult,
    MemorySearchManager, ModelProvider, PluginSecurityPolicy, RunOutcome, SessionDescriptor,
    SessionMessage, SessionStore, Tool, ToolCallInput, ToolCallResult, ToolRegistry,
};
#[cfg(any(not(feature = "memory_rpc"), feature = "memory_legacy_fallback"))]
use kelvin_memory::MemoryBackendKind;
#[cfg(any(not(feature = "memory_rpc"), feature = "memory_legacy_fallback"))]
use kelvin_memory::MemoryFactory;
#[cfg(feature = "memory_rpc")]
use kelvin_memory_client::{MemoryClientConfig, RpcMemoryManager};

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
}

impl KelvinSdkConfig {
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KelvinSdkModelSelection {
    Echo,
    InstalledPlugin { plugin_id: String },
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

#[derive(Default)]
struct InMemorySessionStore {
    sessions: RwLock<HashMap<String, SessionDescriptor>>,
    messages: RwLock<HashMap<String, Vec<SessionMessage>>>,
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn upsert_session(&self, session: SessionDescriptor) -> KelvinResult<()> {
        self.sessions
            .write()
            .await
            .insert(session.session_id.clone(), session);
        Ok(())
    }

    async fn get_session(&self, session_id: &str) -> KelvinResult<Option<SessionDescriptor>> {
        Ok(self.sessions.read().await.get(session_id).cloned())
    }

    async fn append_message(&self, session_id: &str, message: SessionMessage) -> KelvinResult<()> {
        self.messages
            .write()
            .await
            .entry(session_id.to_string())
            .or_default()
            .push(message);
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

pub async fn run_with_sdk(config: KelvinSdkConfig) -> KelvinResult<KelvinRunSummary> {
    let session_store = Arc::new(InMemorySessionStore::default());
    let event_sink = Arc::new(StdoutEventSink);

    let builtin_tools = Arc::new(HashMapToolRegistry::default());
    builtin_tools.register(TimeTool);
    builtin_tools.register(StaticTextTool::new(
        "hello_tool",
        "Hello from Kelvin SDK built-in tools.",
    ));

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

    let cli_plugin_tool = installed_tools.get("kelvin_cli").ok_or_else(|| {
        KelvinError::NotFound(
            "required plugin tool 'kelvin_cli' not found; install it with scripts/install-kelvin-cli-plugin.sh"
                .to_string(),
        )
    })?;

    let run_id = format!("run-{}", now_ms());
    let cli_preflight = cli_plugin_tool
        .call(ToolCallInput {
            run_id: run_id.clone(),
            session_id: config.session_id.clone(),
            workspace_dir: config.workspace_dir.to_string_lossy().to_string(),
            arguments: json!({"prompt": config.prompt}),
        })
        .await?
        .summary;

    let tools: Arc<dyn ToolRegistry> = Arc::new(CombinedToolRegistry::new(vec![
        installed_tools,
        builtin_tools,
    ]));

    #[cfg(feature = "memory_rpc")]
    let memory: Arc<dyn MemorySearchManager> = {
        let mut rpc_cfg = MemoryClientConfig::from_env();
        rpc_cfg.workspace_id = config.workspace_dir.to_string_lossy().to_string();
        rpc_cfg.session_id = config.session_id.clone();
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

    let model: Arc<dyn ModelProvider> = match &config.model_provider {
        KelvinSdkModelSelection::Echo => Arc::new(EchoModelProvider::new("kelvin", "echo-v1")),
        KelvinSdkModelSelection::InstalledPlugin { plugin_id } => {
            if !config.load_installed_plugins {
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
                })?
        }
    };
    let brain = Arc::new(KelvinBrain::new(
        session_store,
        memory,
        model,
        tools,
        event_sink,
    ));

    let runtime = CoreRuntime::new(brain);
    let request = AgentRunRequest {
        run_id: run_id.clone(),
        session_id: config.session_id.clone(),
        session_key: config.session_id,
        workspace_dir: config.workspace_dir.to_string_lossy().to_string(),
        prompt: config.prompt,
        extra_system_prompt: config.system_prompt,
        timeout_ms: Some(config.timeout_ms),
        memory_query: None,
    };

    let accepted = runtime.submit(request).await?;
    println!(
        "accepted run: {} at {}",
        accepted.run_id, accepted.accepted_at_ms
    );

    match runtime
        .wait_for_outcome(&run_id, config.timeout_ms.saturating_add(5_000))
        .await?
    {
        RunOutcome::Completed(result) => Ok(KelvinRunSummary {
            run_id,
            accepted_at_ms: accepted.accepted_at_ms,
            provider: result.meta.provider,
            model: result.meta.model,
            duration_ms: result.meta.duration_ms,
            payloads: result.payloads.into_iter().map(|item| item.text).collect(),
            loaded_installed_plugins,
            cli_plugin_preflight: cli_preflight,
        }),
        RunOutcome::Failed(error) => Err(KelvinError::Backend(format!("run failed: {error}"))),
        RunOutcome::Timeout => Err(KelvinError::Timeout(
            "timed out waiting for run result".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use mockito::Server;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::{run_with_sdk, KelvinCliMemoryMode, KelvinSdkConfig, KelvinSdkModelSelection};

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const TEST_PUBLISHER_ID: &str = "kelvin_sdk_test";
    const TEST_SIGNING_KEY_BYTES: [u8; 32] = [31_u8; 32];

    fn unique_workspace() -> std::path::PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!("kelvin-sdk-test-{millis}"));
        std::fs::create_dir_all(&path).expect("create workspace");
        path
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

    fn model_test_wasm() -> Vec<u8> {
        parse_wat(
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
        )
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

    fn seed_openai_plugin_for_mock_host(
        plugin_home: &Path,
        trust_policy_path: &Path,
        allow_host: &str,
    ) {
        let signing_key = test_signing_key();
        let wasm_bytes = model_test_wasm();
        let plugin_id = "acme.openai";
        write_signed_plugin(
            plugin_home,
            plugin_id,
            "0.1.0",
            "kelvin_openai.wasm",
            &wasm_bytes,
            json!({
                "id": plugin_id,
                "name": "Acme OpenAI Plugin",
                "version": "0.1.0",
                "api_version": "1.0.0",
                "description": "test openai model plugin",
                "homepage": "https://example.com/openai-plugin",
                "capabilities": ["model_provider"],
                "experimental": false,
                "min_core_version": "0.1.0",
                "max_core_version": null,
                "runtime": "wasm_model_v1",
                "provider_name": "openai",
                "model_name": "gpt-4.1-mini",
                "entrypoint": "kelvin_openai.wasm",
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
}
