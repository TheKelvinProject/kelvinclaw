use std::sync::{Arc, Barrier};
use std::thread;

use async_trait::async_trait;

use kelvin_core::{
    check_plugin_compatibility, InMemoryPluginRegistry, KelvinResult, PluginCapability,
    PluginFactory, PluginManifest, PluginRegistry, PluginSecurityPolicy, SdkToolRegistry, Tool,
    ToolCallInput, ToolCallResult, ToolRegistry, KELVIN_CORE_API_VERSION,
};

fn manifest_with_caps(id: &str, capabilities: Vec<PluginCapability>) -> PluginManifest {
    PluginManifest {
        id: id.to_string(),
        name: format!("Plugin {id}"),
        version: "1.0.0".to_string(),
        api_version: KELVIN_CORE_API_VERSION.to_string(),
        description: None,
        homepage: None,
        capabilities,
        experimental: false,
        min_core_version: Some("0.1.0".to_string()),
        max_core_version: None,
    }
}

struct NamedTool {
    name: String,
}

impl NamedTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Tool for NamedTool {
    fn name(&self) -> &str {
        &self.name
    }

    async fn call(&self, _input: ToolCallInput) -> KelvinResult<ToolCallResult> {
        Ok(ToolCallResult {
            summary: format!("{}:ok", self.name),
            output: Some("ok".to_string()),
            visible_text: Some("ok".to_string()),
            is_error: false,
        })
    }
}

struct StaticPlugin {
    manifest: PluginManifest,
    tool: Option<Arc<dyn Tool>>,
}

impl StaticPlugin {
    fn new(manifest: PluginManifest, tool: Option<Arc<dyn Tool>>) -> Self {
        Self { manifest, tool }
    }
}

impl PluginFactory for StaticPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn tool(&self) -> Option<Arc<dyn Tool>> {
        self.tool.clone()
    }
}

#[test]
fn compatibility_rejects_experimental_plugin_by_default() {
    let mut manifest = manifest_with_caps("acme.experimental", vec![]);
    manifest.experimental = true;

    let report = check_plugin_compatibility(&manifest, "0.1.0", &PluginSecurityPolicy::default());
    assert!(!report.compatible);
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("experimental")));
}

#[test]
fn compatibility_rejects_api_major_mismatch() {
    let mut manifest = manifest_with_caps("acme.api-mismatch", vec![]);
    manifest.api_version = "2.0.0".to_string();

    let report = check_plugin_compatibility(&manifest, "0.1.0", &PluginSecurityPolicy::default());
    assert!(!report.compatible);
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("api version mismatch")));
}

#[test]
fn compatibility_rejects_invalid_core_version_input() {
    let manifest = manifest_with_caps("acme.bad-core-version", vec![]);
    let report =
        check_plugin_compatibility(&manifest, "not-a-semver", &PluginSecurityPolicy::default());
    assert!(!report.compatible);
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("invalid core version")));
}

#[test]
fn compatibility_rejects_multiple_privileged_capabilities_without_opt_in() {
    let manifest = manifest_with_caps(
        "acme.privileged",
        vec![
            PluginCapability::FsWrite,
            PluginCapability::CommandExecution,
            PluginCapability::NetworkEgress,
        ],
    );

    let denied = check_plugin_compatibility(&manifest, "0.1.0", &PluginSecurityPolicy::default());
    assert!(!denied.compatible);
    assert!(denied
        .reasons
        .iter()
        .any(|reason| reason.contains("filesystem write")));
    assert!(denied
        .reasons
        .iter()
        .any(|reason| reason.contains("command execution")));
    assert!(denied
        .reasons
        .iter()
        .any(|reason| reason.contains("network egress")));

    let allowed = check_plugin_compatibility(
        &manifest,
        "0.1.0",
        &PluginSecurityPolicy {
            allow_network_egress: true,
            allow_fs_write: true,
            allow_command_execution: true,
            ..Default::default()
        },
    );
    assert!(allowed.compatible, "{}", allowed.reasons.join("; "));
}

#[test]
fn registry_get_returns_none_for_unknown_plugin() {
    let registry = InMemoryPluginRegistry::new();
    assert!(registry.get("does.not.exist").is_none());
}

#[test]
fn sdk_tool_registry_rejects_tool_without_tool_provider_capability() {
    let registry = InMemoryPluginRegistry::new();
    let plugin = Arc::new(StaticPlugin::new(
        manifest_with_caps("acme.hidden-tool", vec![]),
        Some(Arc::new(NamedTool::new("hidden"))),
    ));
    registry
        .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
        .expect("register");

    let err = match SdkToolRegistry::from_plugin_registry(&registry) {
        Ok(_) => panic!("build should fail"),
        Err(err) => err,
    };
    assert!(err
        .to_string()
        .contains("missing 'tool_provider' capability"));
}

#[test]
fn sdk_tool_registry_ignores_plugins_without_tools() {
    let registry = InMemoryPluginRegistry::new();
    let plugin = Arc::new(StaticPlugin::new(
        manifest_with_caps("acme.metadata-only", vec![]),
        None,
    ));
    registry
        .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
        .expect("register");

    let tools = SdkToolRegistry::from_plugin_registry(&registry).expect("build");
    assert!(tools.names().is_empty());
}

#[test]
fn sdk_tool_registry_names_are_sorted_for_stability() {
    let registry = InMemoryPluginRegistry::new();
    let alpha = Arc::new(StaticPlugin::new(
        manifest_with_caps("acme.alpha", vec![PluginCapability::ToolProvider]),
        Some(Arc::new(NamedTool::new("alpha"))),
    ));
    let zeta = Arc::new(StaticPlugin::new(
        manifest_with_caps("acme.zeta", vec![PluginCapability::ToolProvider]),
        Some(Arc::new(NamedTool::new("zeta"))),
    ));

    registry
        .register(zeta, "0.1.0", &PluginSecurityPolicy::default())
        .expect("register zeta");
    registry
        .register(alpha, "0.1.0", &PluginSecurityPolicy::default())
        .expect("register alpha");

    let tools = SdkToolRegistry::from_plugin_registry(&registry).expect("build");
    assert_eq!(tools.names(), vec!["alpha".to_string(), "zeta".to_string()]);
}

#[test]
fn registry_concurrent_duplicate_registration_allows_only_one_success() {
    let registry = Arc::new(InMemoryPluginRegistry::new());
    let barrier = Arc::new(Barrier::new(2));

    let plugin = Arc::new(StaticPlugin::new(
        manifest_with_caps("acme.race", vec![PluginCapability::ToolProvider]),
        Some(Arc::new(NamedTool::new("race-tool"))),
    ));

    let mut handles = Vec::new();
    for _ in 0..2 {
        let registry = registry.clone();
        let barrier = barrier.clone();
        let plugin = plugin.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            registry
                .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
                .is_ok()
        }));
    }

    let successes = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread join"))
        .filter(|ok| *ok)
        .count();

    assert_eq!(successes, 1, "exactly one register should succeed");
    assert_eq!(
        registry.manifests().len(),
        1,
        "registry must contain one plugin"
    );
}
