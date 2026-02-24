use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use kelvin_core::{
    KelvinError, KelvinResult, PluginCapability, PluginFactory, PluginManifest, Tool,
    ToolCallInput, ToolCallResult, KELVIN_CORE_API_VERSION,
};
use kelvin_wasm::{ClawCall, SandboxPolicy, SandboxPreset, WasmSkillHost};

const DEFAULT_MEMORY_APPEND_PATH: &str = "memory/skill-events.md";
pub const WASM_SKILL_PLUGIN_ID: &str = "kelvin.wasm_skill";
pub const WASM_SKILL_PLUGIN_NAME: &str = "Kelvin WASM Skill Tool";

#[derive(Clone)]
pub struct WasmSkillTool {
    name: String,
    host: Arc<WasmSkillHost>,
    default_policy: SandboxPolicy,
    default_memory_append_path: String,
}

impl WasmSkillTool {
    pub fn new(
        name: impl Into<String>,
        host: Arc<WasmSkillHost>,
        default_policy: SandboxPolicy,
    ) -> Self {
        Self {
            name: name.into(),
            host,
            default_policy,
            default_memory_append_path: DEFAULT_MEMORY_APPEND_PATH.to_string(),
        }
    }

    fn require_args_object<'a>(
        &self,
        value: &'a Value,
    ) -> KelvinResult<&'a serde_json::Map<String, Value>> {
        value.as_object().ok_or_else(|| {
            KelvinError::InvalidInput(format!("{} tool expects JSON object arguments", self.name))
        })
    }

    fn require_string(
        &self,
        args: &serde_json::Map<String, Value>,
        key: &str,
    ) -> KelvinResult<String> {
        let value = args.get(key).ok_or_else(|| {
            KelvinError::InvalidInput(format!("{} tool requires '{key}' argument", self.name))
        })?;
        value.as_str().map(str::to_string).ok_or_else(|| {
            KelvinError::InvalidInput(format!(
                "{} tool argument '{key}' must be a string",
                self.name
            ))
        })
    }

    fn optional_string(
        &self,
        args: &serde_json::Map<String, Value>,
        key: &str,
    ) -> KelvinResult<Option<String>> {
        match args.get(key) {
            None => Ok(None),
            Some(value) => value.as_str().map(|v| Some(v.to_string())).ok_or_else(|| {
                KelvinError::InvalidInput(format!(
                    "{} tool argument '{key}' must be a string",
                    self.name
                ))
            }),
        }
    }

    fn optional_bool(
        &self,
        args: &serde_json::Map<String, Value>,
        key: &str,
    ) -> KelvinResult<Option<bool>> {
        match args.get(key) {
            None => Ok(None),
            Some(value) => value
                .as_bool()
                .map(Some)
                .ok_or_else(|| KelvinError::InvalidInput(format!("'{key}' must be a boolean"))),
        }
    }

    fn optional_u64(
        &self,
        args: &serde_json::Map<String, Value>,
        key: &str,
    ) -> KelvinResult<Option<u64>> {
        match args.get(key) {
            None => Ok(None),
            Some(value) => value
                .as_u64()
                .map(Some)
                .ok_or_else(|| KelvinError::InvalidInput(format!("'{key}' must be a u64"))),
        }
    }

    fn optional_usize(
        &self,
        args: &serde_json::Map<String, Value>,
        key: &str,
    ) -> KelvinResult<Option<usize>> {
        match args.get(key) {
            None => Ok(None),
            Some(value) => {
                let Some(raw) = value.as_u64() else {
                    return Err(KelvinError::InvalidInput(format!(
                        "'{key}' must be a usize"
                    )));
                };
                usize::try_from(raw)
                    .map(Some)
                    .map_err(|_| KelvinError::InvalidInput(format!("'{key}' exceeds usize")))
            }
        }
    }

    fn sanitize_rel_path(&self, raw: &str, field: &str) -> KelvinResult<String> {
        let normalized = raw.trim().replace('\\', "/");
        if normalized.is_empty() {
            return Err(KelvinError::InvalidInput(format!(
                "'{field}' must not be empty"
            )));
        }
        if Path::new(&normalized).is_absolute() || normalized.starts_with('/') {
            return Err(KelvinError::InvalidInput(format!(
                "'{field}' must be a relative path"
            )));
        }
        let path = Path::new(&normalized);
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(KelvinError::InvalidInput(format!(
                "'{field}' path traversal is not allowed"
            )));
        }
        Ok(normalized)
    }

    fn validate_memory_path_scope(&self, memory_rel_path: &str) -> KelvinResult<()> {
        let is_memory_root = memory_rel_path == "MEMORY.md";
        let is_memory_daily =
            memory_rel_path.starts_with("memory/") && memory_rel_path.ends_with(".md");
        if !is_memory_root && !is_memory_daily {
            return Err(KelvinError::InvalidInput(
                "memory append path must be MEMORY.md or memory/*.md".to_string(),
            ));
        }
        Ok(())
    }

    fn resolve_policy(
        &self,
        args: &serde_json::Map<String, Value>,
        default_policy: SandboxPolicy,
    ) -> KelvinResult<SandboxPolicy> {
        let mut policy = if let Some(raw) = self.optional_string(args, "policy_preset")? {
            SandboxPreset::parse(&raw)
                .ok_or_else(|| KelvinError::InvalidInput(format!("unknown policy preset: {raw}")))?
                .policy()
        } else {
            default_policy
        };

        if let Some(value) = self.optional_bool(args, "allow_move_servo")? {
            policy.allow_move_servo = value;
        }
        if let Some(value) = self.optional_bool(args, "allow_fs_read")? {
            policy.allow_fs_read = value;
        }
        if let Some(value) = self.optional_bool(args, "allow_network_send")? {
            policy.allow_network_send = value;
        }
        if let Some(value) = self.optional_usize(args, "max_module_bytes")? {
            policy.max_module_bytes = value;
        }
        if let Some(value) = self.optional_u64(args, "fuel_budget")? {
            policy.fuel_budget = value;
        }

        Ok(policy)
    }
}

impl Default for WasmSkillTool {
    fn default() -> Self {
        Self::new(
            "wasm_skill",
            Arc::new(WasmSkillHost::new()),
            SandboxPolicy::locked_down(),
        )
    }
}

#[derive(Clone)]
pub struct WasmSkillPlugin {
    manifest: PluginManifest,
    tool: Arc<WasmSkillTool>,
}

impl WasmSkillPlugin {
    pub fn new(tool: Arc<WasmSkillTool>) -> Self {
        Self {
            manifest: Self::default_manifest(),
            tool,
        }
    }

    pub fn default_manifest() -> PluginManifest {
        PluginManifest {
            id: WASM_SKILL_PLUGIN_ID.to_string(),
            name: WASM_SKILL_PLUGIN_NAME.to_string(),
            version: "0.1.0".to_string(),
            api_version: KELVIN_CORE_API_VERSION.to_string(),
            description: Some(
                "Sandboxed WebAssembly skill execution with workspace-scoped memory append."
                    .to_string(),
            ),
            homepage: None,
            capabilities: vec![
                PluginCapability::ToolProvider,
                PluginCapability::FsRead,
                PluginCapability::FsWrite,
            ],
            experimental: false,
            min_core_version: Some("0.1.0".to_string()),
            max_core_version: None,
        }
    }
}

impl Default for WasmSkillPlugin {
    fn default() -> Self {
        Self::new(Arc::new(WasmSkillTool::default()))
    }
}

impl PluginFactory for WasmSkillPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn tool(&self) -> Option<Arc<dyn Tool>> {
        Some(self.tool.clone())
    }
}

#[async_trait]
impl Tool for WasmSkillTool {
    fn name(&self) -> &str {
        &self.name
    }

    async fn call(&self, input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        let args = self.require_args_object(&input.arguments)?;
        let wasm_rel_path =
            self.sanitize_rel_path(&self.require_string(args, "wasm_path")?, "wasm_path")?;
        let policy = self.resolve_policy(args, self.default_policy)?;

        let workspace_dir = PathBuf::from(&input.workspace_dir);
        let wasm_path = workspace_dir.join(&wasm_rel_path);
        let execution = self.host.run_file(&wasm_path, policy)?;

        let memory_rel_path = self
            .optional_string(args, "memory_append_path")?
            .unwrap_or_else(|| self.default_memory_append_path.clone());
        let memory_rel_path = self.sanitize_rel_path(&memory_rel_path, "memory_append_path")?;
        self.validate_memory_path_scope(&memory_rel_path)?;

        let memory_entry = self
            .optional_string(args, "memory_entry")?
            .unwrap_or_else(|| {
                format!(
                    "run_id={} exit_code={} calls={}",
                    input.run_id,
                    execution.exit_code,
                    execution
                        .calls
                        .iter()
                        .map(claw_call_label)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            });

        let memory_abs_path = workspace_dir.join(&memory_rel_path);
        if let Some(parent) = memory_abs_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut memory_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&memory_abs_path)?;
        writeln!(memory_file, "{memory_entry}")?;

        let calls_json = execution
            .calls
            .iter()
            .map(claw_call_json)
            .collect::<Vec<_>>();
        let summary = format!(
            "wasm skill exit={} calls={}",
            execution.exit_code,
            calls_json.len()
        );
        let output = json!({
            "wasm_path": wasm_rel_path,
            "memory_path": memory_rel_path,
            "exit_code": execution.exit_code,
            "calls": calls_json,
        });

        Ok(ToolCallResult {
            summary: summary.clone(),
            output: Some(output.to_string()),
            visible_text: Some(summary),
            is_error: false,
        })
    }
}

fn claw_call_label(call: &ClawCall) -> String {
    match call {
        ClawCall::SendMessage { message_code } => format!("send_message({message_code})"),
        ClawCall::MoveServo { channel, position } => format!("move_servo({channel},{position})"),
        ClawCall::FsRead { handle } => format!("fs_read({handle})"),
        ClawCall::NetworkSend { packet } => format!("network_send({packet})"),
    }
}

fn claw_call_json(call: &ClawCall) -> Value {
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
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use kelvin_core::Tool;

    use super::WasmSkillTool;

    fn unique_test_workspace() -> std::path::PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("kelvin-wasm-tool-{millis}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_wasm(workspace: &Path, rel_path: &str, wat_src: &str) {
        let bytes = wat::parse_str(wat_src).expect("parse wat");
        let abs = workspace.join(rel_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).expect("create skill dir");
        }
        std::fs::write(abs, bytes).expect("write wasm file");
    }

    #[tokio::test]
    async fn runs_wasm_and_appends_memory_entry() {
        let workspace = unique_test_workspace();
        write_wasm(
            &workspace,
            "skills/echo.wasm",
            r#"
            (module
              (import "claw" "send_message" (func $send_message (param i32) (result i32)))
              (func (export "run") (result i32)
                i32.const 42
                call $send_message
                drop
                i32.const 0
              )
            )
            "#,
        );

        let tool = WasmSkillTool::default();
        let result = tool
            .call(kelvin_core::ToolCallInput {
                run_id: "run-1".to_string(),
                session_id: "session-1".to_string(),
                workspace_dir: workspace.to_string_lossy().to_string(),
                arguments: json!({
                    "wasm_path": "skills/echo.wasm",
                    "memory_append_path": "memory/mvp.md",
                    "memory_entry": "mvp skill executed",
                    "policy_preset": "locked_down"
                }),
            })
            .await
            .expect("tool call");

        assert!(!result.is_error);
        let memory_text =
            std::fs::read_to_string(workspace.join("memory/mvp.md")).expect("memory file");
        assert!(memory_text.contains("mvp skill executed"));
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        let workspace = unique_test_workspace();
        let tool = WasmSkillTool::new(
            "wasm_skill",
            Arc::new(kelvin_wasm::WasmSkillHost::new()),
            kelvin_wasm::SandboxPolicy::locked_down(),
        );

        let error = tool
            .call(kelvin_core::ToolCallInput {
                run_id: "run-1".to_string(),
                session_id: "session-1".to_string(),
                workspace_dir: workspace.to_string_lossy().to_string(),
                arguments: json!({
                    "wasm_path": "../escape.wasm"
                }),
            })
            .await
            .expect_err("path traversal should fail");
        assert!(error.to_string().contains("path traversal"));
    }
}
