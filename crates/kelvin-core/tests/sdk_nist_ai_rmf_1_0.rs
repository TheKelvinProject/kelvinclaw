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
        description: Some("NIST AI RMF SDK stress test plugin".to_string()),
        homepage: Some("https://example.com/plugin".to_string()),
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
    fn with_tool(manifest: PluginManifest, tool_name: &str) -> Self {
        Self {
            manifest,
            tool: Some(Arc::new(NamedTool::new(tool_name))),
        }
    }

    fn metadata_only(manifest: PluginManifest) -> Self {
        Self {
            manifest,
            tool: None,
        }
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

// GOVERN: policy, roles, and organizational accountability.
#[test]
fn govern_default_policy_denies_privileged_and_experimental_capabilities() {
    let mut manifest = manifest_with_caps(
        "acme.high-risk",
        vec![
            PluginCapability::FsRead,
            PluginCapability::FsWrite,
            PluginCapability::NetworkEgress,
            PluginCapability::CommandExecution,
        ],
    );
    manifest.experimental = true;

    let report = check_plugin_compatibility(&manifest, "0.1.0", &PluginSecurityPolicy::default());
    assert!(!report.compatible);
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("experimental")));
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("filesystem read")));
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("filesystem write")));
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("network egress")));
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("command execution")));
}

#[test]
fn govern_explicit_policy_can_allow_documented_privileges() {
    let mut manifest = manifest_with_caps(
        "acme.allowable",
        vec![
            PluginCapability::FsRead,
            PluginCapability::FsWrite,
            PluginCapability::NetworkEgress,
            PluginCapability::CommandExecution,
        ],
    );
    manifest.experimental = true;

    let report = check_plugin_compatibility(
        &manifest,
        "0.1.0",
        &PluginSecurityPolicy {
            allow_experimental: true,
            allow_fs_read: true,
            allow_fs_write: true,
            allow_network_egress: true,
            allow_command_execution: true,
        },
    );
    assert!(report.compatible, "{}", report.reasons.join("; "));
}

// MAP: context, intended use, and capability boundary mapping.
#[test]
fn map_manifest_rejects_untraceable_metadata_shapes() {
    let mut invalid_homepage = manifest_with_caps("acme.meta-1", vec![]);
    invalid_homepage.homepage = Some("javascript:alert(1)".to_string());
    let err = invalid_homepage
        .validate()
        .expect_err("non-http homepage should fail");
    assert!(err.to_string().contains("http:// or https://"));

    let mut invalid_name = manifest_with_caps("acme.meta-2", vec![]);
    invalid_name.name = "name-with\ncontrol".to_string();
    let err = invalid_name
        .validate()
        .expect_err("control characters should fail");
    assert!(err.to_string().contains("control characters"));
}

#[test]
fn map_tool_provider_capability_must_match_actual_tool_export() {
    let registry = InMemoryPluginRegistry::new();

    // Plugin exposes a tool but does not declare tool_provider.
    let hidden_tool = Arc::new(StaticPlugin::with_tool(
        manifest_with_caps("acme.hidden-tool", vec![]),
        "hidden_tool",
    ));
    registry
        .register(hidden_tool, "0.1.0", &PluginSecurityPolicy::default())
        .expect("register");

    let err = match SdkToolRegistry::from_plugin_registry(&registry) {
        Ok(_) => panic!("projection should fail"),
        Err(err) => err,
    };
    assert!(err
        .to_string()
        .contains("missing 'tool_provider' capability"));
}

#[test]
fn map_declared_tool_provider_requires_concrete_tool() {
    let registry = InMemoryPluginRegistry::new();
    let plugin = Arc::new(StaticPlugin::metadata_only(manifest_with_caps(
        "acme.declared-no-tool",
        vec![PluginCapability::ToolProvider],
    )));
    registry
        .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
        .expect("register");

    let err = match SdkToolRegistry::from_plugin_registry(&registry) {
        Ok(_) => panic!("projection should fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("returned no tool"));
}

// MEASURE: measurable, reproducible signals and diagnostics.
#[test]
fn measure_compatibility_report_is_actionable_and_specific() {
    let manifest = manifest_with_caps(
        "acme.measure",
        vec![
            PluginCapability::FsRead,
            PluginCapability::FsWrite,
            PluginCapability::NetworkEgress,
            PluginCapability::CommandExecution,
        ],
    );
    let report = check_plugin_compatibility(&manifest, "0.1.0", &PluginSecurityPolicy::default());
    assert!(!report.compatible);
    assert!(
        report.reasons.len() >= 4,
        "expected multi-signal diagnostics"
    );
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("filesystem read")));
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("filesystem write")));
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("network egress")));
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("command execution")));
}

#[test]
fn measure_tool_projection_order_is_deterministic() {
    let registry = InMemoryPluginRegistry::new();
    for (id, tool_name) in [
        ("acme.zeta", "tool_zeta"),
        ("acme.alpha", "tool_alpha"),
        ("acme.beta", "tool_beta"),
    ] {
        let plugin = Arc::new(StaticPlugin::with_tool(
            manifest_with_caps(id, vec![PluginCapability::ToolProvider]),
            tool_name,
        ));
        registry
            .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
            .expect("register");
    }

    let tools = SdkToolRegistry::from_plugin_registry(&registry).expect("projection");
    assert_eq!(
        tools.names(),
        vec![
            "tool_alpha".to_string(),
            "tool_beta".to_string(),
            "tool_zeta".to_string()
        ]
    );
}

#[test]
fn measure_duplicate_capabilities_are_rejected_for_clean_metrics() {
    let manifest = manifest_with_caps(
        "acme.duplicate-capabilities",
        vec![
            PluginCapability::ToolProvider,
            PluginCapability::ToolProvider,
        ],
    );
    let err = manifest
        .validate()
        .expect_err("duplicate capabilities should fail");
    assert!(err.to_string().contains("duplicate value"));
}

// MANAGE: response, fallback, and operational resilience.
#[test]
fn manage_duplicate_registration_prevents_state_corruption() {
    let registry = InMemoryPluginRegistry::new();
    let plugin_a = Arc::new(StaticPlugin::metadata_only(manifest_with_caps(
        "acme.identity",
        vec![],
    )));
    let plugin_b = Arc::new(StaticPlugin::metadata_only(manifest_with_caps(
        "acme.identity",
        vec![],
    )));

    registry
        .register(plugin_a, "0.1.0", &PluginSecurityPolicy::default())
        .expect("first register");
    let err = registry
        .register(plugin_b, "0.1.0", &PluginSecurityPolicy::default())
        .expect_err("duplicate id should fail");
    assert!(err.to_string().contains("already registered"));
}

#[test]
fn manage_unknown_lookup_is_safe_and_non_panicking() {
    let registry = InMemoryPluginRegistry::new();
    assert!(registry.get("acme.missing").is_none());
}

#[test]
fn manage_concurrent_duplicate_registration_allows_one_winner() {
    let registry = Arc::new(InMemoryPluginRegistry::new());
    let barrier = Arc::new(Barrier::new(2));
    let plugin = Arc::new(StaticPlugin::metadata_only(manifest_with_caps(
        "acme.race",
        vec![],
    )));

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

    let successful = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread join"))
        .filter(|ok| *ok)
        .count();

    assert_eq!(successful, 1, "exactly one register should succeed");
    assert_eq!(
        registry.manifests().len(),
        1,
        "only one plugin should exist"
    );
}

#[test]
fn manage_large_registry_projection_remains_stable() {
    let registry = InMemoryPluginRegistry::new();
    for idx in 0..300 {
        let plugin_id = format!("acme.bulk.{idx:04}");
        let tool_name = format!("tool_{idx:04}");
        let plugin = Arc::new(StaticPlugin::with_tool(
            manifest_with_caps(&plugin_id, vec![PluginCapability::ToolProvider]),
            &tool_name,
        ));
        registry
            .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
            .expect("register");
    }

    let tools = SdkToolRegistry::from_plugin_registry(&registry).expect("projection");
    let names = tools.names();
    assert_eq!(names.len(), 300);
    assert_eq!(names.first().map(String::as_str), Some("tool_0000"));
    assert_eq!(names.last().map(String::as_str), Some("tool_0299"));
}
