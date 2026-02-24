use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{
    EventSink, KelvinError, KelvinResult, MemorySearchManager, ModelProvider, SessionStore, Tool,
    ToolRegistry,
};

pub const KELVIN_CORE_SDK_NAME: &str = "Kelvin Core";
pub const KELVIN_CORE_API_VERSION: &str = "1.0.0";
pub const MAX_PLUGIN_ID_LEN: usize = 128;
pub const MAX_PLUGIN_NAME_LEN: usize = 128;
pub const MAX_PLUGIN_DESCRIPTION_LEN: usize = 4_096;
pub const MAX_PLUGIN_HOMEPAGE_LEN: usize = 2_048;
pub const MAX_PLUGIN_CAPABILITIES: usize = 32;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    ModelProvider,
    MemoryProvider,
    SessionStore,
    EventSink,
    ToolProvider,
    FsRead,
    FsWrite,
    NetworkEgress,
    CommandExecution,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub capabilities: Vec<PluginCapability>,
    pub experimental: bool,
    pub min_core_version: Option<String>,
    pub max_core_version: Option<String>,
}

impl PluginManifest {
    pub fn validate(&self) -> KelvinResult<()> {
        validate_plugin_id(&self.id)?;
        validate_plugin_name(&self.name)?;
        validate_semver("plugin version", &self.version)?;
        validate_semver("api version", &self.api_version)?;
        validate_optional_text_field(
            "plugin description",
            self.description.as_deref(),
            MAX_PLUGIN_DESCRIPTION_LEN,
        )?;
        validate_homepage(self.homepage.as_deref())?;
        validate_capabilities(&self.capabilities)?;

        if let Some(min) = &self.min_core_version {
            validate_semver("min core version", min)?;
        }
        if let Some(max) = &self.max_core_version {
            validate_semver("max core version", max)?;
        }

        Ok(())
    }
}

fn validate_semver(label: &str, value: &str) -> KelvinResult<()> {
    if value.trim().is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    Version::parse(value).map_err(|err| {
        let shown = preview(value, 64);
        KelvinError::InvalidInput(format!(
            "{label} must be valid semver, got '{shown}': {err}"
        ))
    })?;
    Ok(())
}

fn validate_plugin_id(value: &str) -> KelvinResult<()> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return Err(KelvinError::InvalidInput(
            "plugin id must not be empty".to_string(),
        ));
    }
    if cleaned.chars().count() > MAX_PLUGIN_ID_LEN {
        return Err(KelvinError::InvalidInput(format!(
            "plugin id exceeds max length {MAX_PLUGIN_ID_LEN}"
        )));
    }
    if cleaned.chars().any(|ch| ch.is_control()) {
        return Err(KelvinError::InvalidInput(
            "plugin id must not include control characters".to_string(),
        ));
    }
    if !cleaned
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        let shown = preview(cleaned, 64);
        return Err(KelvinError::InvalidInput(format!(
            "plugin id has invalid characters: {shown}"
        )));
    }
    Ok(())
}

fn validate_plugin_name(value: &str) -> KelvinResult<()> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return Err(KelvinError::InvalidInput(
            "plugin name must not be empty".to_string(),
        ));
    }
    if cleaned.chars().count() > MAX_PLUGIN_NAME_LEN {
        return Err(KelvinError::InvalidInput(format!(
            "plugin name exceeds max length {MAX_PLUGIN_NAME_LEN}"
        )));
    }
    if cleaned.chars().any(|ch| ch.is_control()) {
        return Err(KelvinError::InvalidInput(
            "plugin name must not include control characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_optional_text_field(
    label: &str,
    value: Option<&str>,
    max_len: usize,
) -> KelvinResult<()> {
    let Some(raw) = value else {
        return Ok(());
    };
    let cleaned = raw.trim();
    if cleaned.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    if cleaned.chars().count() > max_len {
        return Err(KelvinError::InvalidInput(format!(
            "{label} exceeds max length {max_len}"
        )));
    }
    if cleaned.chars().any(|ch| ch.is_control()) {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not include control characters"
        )));
    }
    Ok(())
}

fn validate_homepage(value: Option<&str>) -> KelvinResult<()> {
    let Some(raw) = value else {
        return Ok(());
    };
    let cleaned = raw.trim();
    if cleaned.is_empty() {
        return Err(KelvinError::InvalidInput(
            "plugin homepage must not be empty".to_string(),
        ));
    }
    if cleaned.chars().count() > MAX_PLUGIN_HOMEPAGE_LEN {
        return Err(KelvinError::InvalidInput(format!(
            "plugin homepage exceeds max length {MAX_PLUGIN_HOMEPAGE_LEN}"
        )));
    }
    if cleaned
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(KelvinError::InvalidInput(
            "plugin homepage must not include whitespace/control characters".to_string(),
        ));
    }
    if !(cleaned.starts_with("https://") || cleaned.starts_with("http://")) {
        return Err(KelvinError::InvalidInput(
            "plugin homepage must use http:// or https://".to_string(),
        ));
    }
    Ok(())
}

fn validate_capabilities(capabilities: &[PluginCapability]) -> KelvinResult<()> {
    if capabilities.len() > MAX_PLUGIN_CAPABILITIES {
        return Err(KelvinError::InvalidInput(format!(
            "plugin capabilities exceed max length {MAX_PLUGIN_CAPABILITIES}"
        )));
    }

    let mut seen = HashSet::new();
    for capability in capabilities {
        if !seen.insert(*capability) {
            return Err(KelvinError::InvalidInput(format!(
                "plugin capabilities contain duplicate value: {capability:?}"
            )));
        }
    }
    Ok(())
}

fn preview(value: &str, max_chars: usize) -> String {
    let mut shown = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            shown.push_str("...");
            return shown;
        }
        shown.push(ch);
    }
    shown
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginSecurityPolicy {
    pub allow_experimental: bool,
    pub allow_fs_read: bool,
    pub allow_network_egress: bool,
    pub allow_fs_write: bool,
    pub allow_command_execution: bool,
}

impl Default for PluginSecurityPolicy {
    fn default() -> Self {
        Self {
            allow_experimental: false,
            allow_fs_read: false,
            allow_network_egress: false,
            allow_fs_write: false,
            allow_command_execution: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCompatibilityReport {
    pub compatible: bool,
    pub reasons: Vec<String>,
}

impl PluginCompatibilityReport {
    pub fn success() -> Self {
        Self {
            compatible: true,
            reasons: Vec::new(),
        }
    }

    pub fn failure(reason: impl Into<String>) -> Self {
        Self {
            compatible: false,
            reasons: vec![reason.into()],
        }
    }
}

pub fn check_plugin_compatibility(
    manifest: &PluginManifest,
    core_version: &str,
    security_policy: &PluginSecurityPolicy,
) -> PluginCompatibilityReport {
    if let Err(err) = manifest.validate() {
        return PluginCompatibilityReport::failure(err.to_string());
    }

    let Ok(core_version) = Version::parse(core_version) else {
        return PluginCompatibilityReport::failure(format!("invalid core version: {core_version}"));
    };

    let mut reasons = Vec::new();
    let plugin_api = Version::parse(&manifest.api_version);
    let core_api = Version::parse(KELVIN_CORE_API_VERSION);
    if let (Ok(plugin_api), Ok(core_api)) = (plugin_api, core_api) {
        if plugin_api.major != core_api.major {
            reasons.push(format!(
                "api version mismatch: plugin {} vs core {}",
                plugin_api, core_api
            ));
        }
    }

    if let Some(min) = &manifest.min_core_version {
        match Version::parse(min) {
            Ok(min_version) if core_version < min_version => reasons.push(format!(
                "core version {} is lower than required minimum {}",
                core_version, min_version
            )),
            Ok(_) => {}
            Err(err) => reasons.push(format!("invalid min_core_version '{min}': {err}")),
        }
    }

    if let Some(max) = &manifest.max_core_version {
        match Version::parse(max) {
            Ok(max_version) if core_version > max_version => reasons.push(format!(
                "core version {} exceeds plugin maximum {}",
                core_version, max_version
            )),
            Ok(_) => {}
            Err(err) => reasons.push(format!("invalid max_core_version '{max}': {err}")),
        }
    }

    if manifest.experimental && !security_policy.allow_experimental {
        reasons.push(format!(
            "plugin '{}' is experimental and policy disallows experimental plugins",
            manifest.id
        ));
    }

    if manifest.capabilities.contains(&PluginCapability::FsRead) && !security_policy.allow_fs_read {
        reasons.push(format!(
            "plugin '{}' requires filesystem read but policy disallows it",
            manifest.id
        ));
    }

    if manifest
        .capabilities
        .contains(&PluginCapability::NetworkEgress)
        && !security_policy.allow_network_egress
    {
        reasons.push(format!(
            "plugin '{}' requires network egress but policy disallows it",
            manifest.id
        ));
    }

    if manifest.capabilities.contains(&PluginCapability::FsWrite) && !security_policy.allow_fs_write
    {
        reasons.push(format!(
            "plugin '{}' requires filesystem write but policy disallows it",
            manifest.id
        ));
    }

    if manifest
        .capabilities
        .contains(&PluginCapability::CommandExecution)
        && !security_policy.allow_command_execution
    {
        reasons.push(format!(
            "plugin '{}' requires command execution but policy disallows it",
            manifest.id
        ));
    }

    if reasons.is_empty() {
        PluginCompatibilityReport::success()
    } else {
        PluginCompatibilityReport {
            compatible: false,
            reasons,
        }
    }
}

pub trait PluginFactory: Send + Sync {
    fn manifest(&self) -> &PluginManifest;

    fn tool(&self) -> Option<Arc<dyn Tool>> {
        None
    }

    fn memory_provider(&self) -> Option<Arc<dyn MemorySearchManager>> {
        None
    }

    fn model_provider(&self) -> Option<Arc<dyn ModelProvider>> {
        None
    }

    fn session_store(&self) -> Option<Arc<dyn SessionStore>> {
        None
    }

    fn event_sink(&self) -> Option<Arc<dyn EventSink>> {
        None
    }
}

pub trait PluginRegistry: Send + Sync {
    fn register(
        &self,
        plugin: Arc<dyn PluginFactory>,
        core_version: &str,
        security_policy: &PluginSecurityPolicy,
    ) -> KelvinResult<()>;

    fn get(&self, plugin_id: &str) -> Option<Arc<dyn PluginFactory>>;

    fn manifests(&self) -> Vec<PluginManifest>;
}

pub struct InMemoryPluginRegistry {
    plugins: RwLock<HashMap<String, Arc<dyn PluginFactory>>>,
}

impl Default for InMemoryPluginRegistry {
    fn default() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
        }
    }
}

impl InMemoryPluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PluginRegistry for InMemoryPluginRegistry {
    fn register(
        &self,
        plugin: Arc<dyn PluginFactory>,
        core_version: &str,
        security_policy: &PluginSecurityPolicy,
    ) -> KelvinResult<()> {
        let manifest = plugin.manifest().clone();
        let report = check_plugin_compatibility(&manifest, core_version, security_policy);
        if !report.compatible {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' rejected: {}",
                manifest.id,
                report.reasons.join("; ")
            )));
        }

        let mut lock = self
            .plugins
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lock.contains_key(&manifest.id) {
            return Err(KelvinError::InvalidInput(format!(
                "plugin '{}' is already registered",
                manifest.id
            )));
        }
        lock.insert(manifest.id.clone(), plugin);
        Ok(())
    }

    fn get(&self, plugin_id: &str) -> Option<Arc<dyn PluginFactory>> {
        self.plugins
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(plugin_id)
            .cloned()
    }

    fn manifests(&self) -> Vec<PluginManifest> {
        self.plugins
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .map(|plugin| plugin.manifest().clone())
            .collect()
    }
}

pub struct SdkToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl SdkToolRegistry {
    pub fn from_plugin_registry(registry: &dyn PluginRegistry) -> KelvinResult<Self> {
        let mut tools = HashMap::new();
        let mut manifests = registry.manifests();
        manifests.sort_by(|left, right| left.id.cmp(&right.id));

        for manifest in manifests {
            let plugin = registry.get(&manifest.id).ok_or_else(|| {
                KelvinError::NotFound(format!(
                    "plugin '{}' disappeared during tool registry build",
                    manifest.id
                ))
            })?;

            let declared_tool_provider = manifest
                .capabilities
                .contains(&PluginCapability::ToolProvider);
            let provided_tool = plugin.tool();

            match (declared_tool_provider, provided_tool) {
                (false, None) => {}
                (false, Some(_)) => {
                    return Err(KelvinError::InvalidInput(format!(
                        "plugin '{}' exposes a tool but is missing '{}' capability",
                        manifest.id, "tool_provider"
                    )));
                }
                (true, None) => {
                    return Err(KelvinError::InvalidInput(format!(
                        "plugin '{}' declares tool capability but returned no tool",
                        manifest.id
                    )));
                }
                (true, Some(tool)) => {
                    let tool_name = tool.name().trim();
                    if tool_name.is_empty() {
                        return Err(KelvinError::InvalidInput(format!(
                            "plugin '{}' returned a tool with empty name",
                            manifest.id
                        )));
                    }
                    if tools.contains_key(tool_name) {
                        return Err(KelvinError::InvalidInput(format!(
                            "duplicate tool name from plugins: {tool_name}"
                        )));
                    }
                    tools.insert(tool_name.to_string(), tool);
                }
            }
        }

        Ok(Self { tools })
    }
}

impl ToolRegistry for SdkToolRegistry {
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    fn names(&self) -> Vec<String> {
        let mut names = self.tools.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::{KelvinResult, Tool, ToolCallInput, ToolCallResult, ToolRegistry};

    use super::{
        check_plugin_compatibility, InMemoryPluginRegistry, PluginCapability, PluginFactory,
        PluginManifest, PluginRegistry, PluginSecurityPolicy, SdkToolRegistry,
        KELVIN_CORE_API_VERSION,
    };

    fn test_manifest() -> PluginManifest {
        PluginManifest {
            id: "acme.echo".to_string(),
            name: "Echo".to_string(),
            version: "1.2.3".to_string(),
            api_version: KELVIN_CORE_API_VERSION.to_string(),
            description: Some("Echo test plugin".to_string()),
            homepage: None,
            capabilities: vec![PluginCapability::ToolProvider],
            experimental: false,
            min_core_version: Some("0.1.0".to_string()),
            max_core_version: None,
        }
    }

    struct EchoTool;

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        async fn call(&self, _input: ToolCallInput) -> KelvinResult<ToolCallResult> {
            Ok(ToolCallResult {
                summary: "ok".to_string(),
                output: Some("ok".to_string()),
                visible_text: Some("ok".to_string()),
                is_error: false,
            })
        }
    }

    struct EchoPlugin {
        manifest: PluginManifest,
    }

    impl PluginFactory for EchoPlugin {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }

        fn tool(&self) -> Option<Arc<dyn Tool>> {
            Some(Arc::new(EchoTool))
        }
    }

    struct EmptyToolPlugin {
        manifest: PluginManifest,
    }

    impl PluginFactory for EmptyToolPlugin {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }
    }

    struct ConflictingToolPlugin {
        manifest: PluginManifest,
    }

    impl PluginFactory for ConflictingToolPlugin {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }

        fn tool(&self) -> Option<Arc<dyn Tool>> {
            Some(Arc::new(EchoTool))
        }
    }

    #[test]
    fn manifest_validation_rejects_invalid_ids() {
        let mut manifest = test_manifest();
        manifest.id = "bad id".to_string();
        let err = manifest.validate().expect_err("invalid id");
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn compatibility_rejects_disallowed_capability() {
        let mut manifest = test_manifest();
        manifest.capabilities.push(PluginCapability::NetworkEgress);
        let policy = PluginSecurityPolicy::default();
        let report = check_plugin_compatibility(&manifest, "0.1.0", &policy);
        assert!(!report.compatible);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason.contains("network egress")));
    }

    #[test]
    fn compatibility_accepts_with_matching_policy() {
        let mut manifest = test_manifest();
        manifest.capabilities.push(PluginCapability::NetworkEgress);
        let policy = PluginSecurityPolicy {
            allow_network_egress: true,
            ..Default::default()
        };
        let report = check_plugin_compatibility(&manifest, "0.1.0", &policy);
        assert!(report.compatible, "{}", report.reasons.join("; "));
    }

    #[test]
    fn registry_registers_and_gets_plugin() {
        let registry = InMemoryPluginRegistry::new();
        let plugin = Arc::new(EchoPlugin {
            manifest: test_manifest(),
        });

        registry
            .register(plugin.clone(), "0.1.0", &PluginSecurityPolicy::default())
            .expect("register");

        let fetched = registry.get("acme.echo").expect("get");
        assert_eq!(fetched.manifest().id, "acme.echo");
        assert_eq!(registry.manifests().len(), 1);
        assert_eq!(fetched.tool().expect("tool").name(), "echo");
    }

    #[test]
    fn registry_rejects_duplicate_plugin_ids() {
        let registry = InMemoryPluginRegistry::new();
        let plugin = Arc::new(EchoPlugin {
            manifest: test_manifest(),
        });
        registry
            .register(plugin.clone(), "0.1.0", &PluginSecurityPolicy::default())
            .expect("first register");
        let err = registry
            .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
            .expect_err("duplicate");
        assert!(err.to_string().contains("already registered"));
    }

    #[test]
    fn registry_rejects_version_out_of_range() {
        let registry = InMemoryPluginRegistry::new();
        let mut manifest = test_manifest();
        manifest.min_core_version = Some("9.0.0".to_string());
        manifest.max_core_version = Some("9.9.9".to_string());
        manifest.description = Some(json!({"note": "test"}).to_string());
        let plugin = Arc::new(EchoPlugin { manifest });
        let err = registry
            .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
            .expect_err("range check");
        assert!(err.to_string().contains("lower than required minimum"));
    }

    #[test]
    fn sdk_tool_registry_builds_from_registered_plugins() {
        let registry = InMemoryPluginRegistry::new();
        let plugin = Arc::new(EchoPlugin {
            manifest: test_manifest(),
        });
        registry
            .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
            .expect("register");

        let tools = SdkToolRegistry::from_plugin_registry(&registry).expect("tool registry");
        assert_eq!(tools.names(), vec!["echo".to_string()]);
        assert!(tools.get("echo").is_some());
    }

    #[test]
    fn sdk_tool_registry_rejects_missing_tool_implementation() {
        let registry = InMemoryPluginRegistry::new();
        let manifest = test_manifest();
        let plugin = Arc::new(EmptyToolPlugin { manifest });
        registry
            .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
            .expect("register");

        let err = match SdkToolRegistry::from_plugin_registry(&registry) {
            Ok(_) => panic!("missing tool should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("returned no tool"));
    }

    #[test]
    fn sdk_tool_registry_rejects_duplicate_tool_names() {
        let registry = InMemoryPluginRegistry::new();
        let first = Arc::new(EchoPlugin {
            manifest: PluginManifest {
                id: "acme.echo.first".to_string(),
                ..test_manifest()
            },
        });
        let second = Arc::new(ConflictingToolPlugin {
            manifest: PluginManifest {
                id: "acme.echo.second".to_string(),
                ..test_manifest()
            },
        });
        registry
            .register(first, "0.1.0", &PluginSecurityPolicy::default())
            .expect("register first");
        registry
            .register(second, "0.1.0", &PluginSecurityPolicy::default())
            .expect("register second");

        let err = match SdkToolRegistry::from_plugin_registry(&registry) {
            Ok(_) => panic!("duplicate tools should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("duplicate tool name"));
    }
}
