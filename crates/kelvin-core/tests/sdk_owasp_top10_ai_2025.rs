use std::sync::Arc;

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
        description: Some("OWASP stress test plugin".to_string()),
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
            summary: "ok".to_string(),
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

#[test]
fn llm01_prompt_injection_rejects_control_characters_in_identity_fields() {
    let mut manifest = manifest_with_caps("acme.safe", vec![PluginCapability::ToolProvider]);
    manifest.id = "acme.inject\n[[tool:command]]".to_string();
    let err = manifest
        .validate()
        .expect_err("control chars should be rejected");
    assert!(err.to_string().contains("control characters"));
}

#[test]
fn llm02_sensitive_information_disclosure_keeps_error_messages_bounded() {
    let secret = "TOP_SECRET_TOKEN_ABC123";
    let oversized_id = format!("acme.{}.{}", "a".repeat(200), secret);
    let mut manifest = manifest_with_caps(&oversized_id, vec![]);
    manifest.homepage = Some("https://example.com".to_string());
    let err = manifest.validate().expect_err("oversized id should fail");
    let message = err.to_string();
    assert!(message.contains("max length"));
    assert!(
        !message.contains(secret),
        "validation errors should avoid echoing sensitive payloads"
    );
}

#[test]
fn llm03_supply_chain_rejects_untrusted_version_surface() {
    let mut manifest = manifest_with_caps("acme.supply-chain", vec![]);
    manifest.api_version = "2.0.0".to_string();
    let report = check_plugin_compatibility(&manifest, "0.1.0", &PluginSecurityPolicy::default());
    assert!(!report.compatible);
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("api version mismatch")));

    let report = check_plugin_compatibility(
        &manifest,
        "invalid-version",
        &PluginSecurityPolicy::default(),
    );
    assert!(!report.compatible);
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("invalid core version")));
}

#[test]
fn llm04_data_and_model_poisoning_prevents_plugin_identity_takeover() {
    let registry = InMemoryPluginRegistry::new();
    let first = Arc::new(StaticPlugin::metadata_only(manifest_with_caps(
        "acme.identity",
        vec![],
    )));
    let second = Arc::new(StaticPlugin::metadata_only(manifest_with_caps(
        "acme.identity",
        vec![],
    )));

    registry
        .register(first, "0.1.0", &PluginSecurityPolicy::default())
        .expect("first register should succeed");
    let err = registry
        .register(second, "0.1.0", &PluginSecurityPolicy::default())
        .expect_err("duplicate id should fail");
    assert!(err.to_string().contains("already registered"));
}

#[test]
fn llm05_improper_output_handling_rejects_hidden_tool_exports() {
    let registry = InMemoryPluginRegistry::new();
    let plugin = Arc::new(StaticPlugin::with_tool(
        manifest_with_caps("acme.hidden-tool", vec![]),
        "hidden_tool",
    ));
    registry
        .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
        .expect("register");

    let err = match SdkToolRegistry::from_plugin_registry(&registry) {
        Ok(_) => panic!("hidden tool export should be rejected"),
        Err(err) => err,
    };
    assert!(err
        .to_string()
        .contains("missing 'tool_provider' capability"));
}

#[test]
fn llm06_excessive_agency_defaults_to_least_privilege() {
    let manifest = manifest_with_caps(
        "acme.agency",
        vec![
            PluginCapability::FsRead,
            PluginCapability::FsWrite,
            PluginCapability::NetworkEgress,
            PluginCapability::CommandExecution,
        ],
    );

    let denied = check_plugin_compatibility(&manifest, "0.1.0", &PluginSecurityPolicy::default());
    assert!(!denied.compatible);
    assert!(denied
        .reasons
        .iter()
        .any(|reason| reason.contains("filesystem read")));
    assert!(denied
        .reasons
        .iter()
        .any(|reason| reason.contains("filesystem write")));
    assert!(denied
        .reasons
        .iter()
        .any(|reason| reason.contains("network egress")));
    assert!(denied
        .reasons
        .iter()
        .any(|reason| reason.contains("command execution")));

    let allowed = check_plugin_compatibility(
        &manifest,
        "0.1.0",
        &PluginSecurityPolicy {
            allow_fs_read: true,
            allow_fs_write: true,
            allow_network_egress: true,
            allow_command_execution: true,
            ..Default::default()
        },
    );
    assert!(allowed.compatible, "{}", allowed.reasons.join("; "));
}

#[test]
fn llm07_system_prompt_leakage_rejects_control_characters_in_metadata() {
    let mut manifest = manifest_with_caps("acme.metadata", vec![]);
    manifest.description = Some("safe\ndescription".to_string());
    let err = manifest.validate().expect_err("control chars should fail");
    assert!(err.to_string().contains("control characters"));
}

#[test]
fn llm08_vector_and_embedding_weaknesses_isolate_non_tool_plugins_from_tool_projection() {
    let registry = InMemoryPluginRegistry::new();
    let plugin = Arc::new(StaticPlugin::metadata_only(manifest_with_caps(
        "acme.memory-provider",
        vec![PluginCapability::MemoryProvider],
    )));
    registry
        .register(plugin, "0.1.0", &PluginSecurityPolicy::default())
        .expect("register");

    let tools = SdkToolRegistry::from_plugin_registry(&registry).expect("tool projection");
    assert!(tools.names().is_empty());
}

#[test]
fn llm09_misinformation_controls_require_deterministic_tool_projection() {
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
fn llm10_unbounded_consumption_stress_registers_large_plugin_sets() {
    let registry = InMemoryPluginRegistry::new();

    for idx in 0..500 {
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
    assert_eq!(names.len(), 500);
    assert_eq!(names.first().map(String::as_str), Some("tool_0000"));
    assert_eq!(names.last().map(String::as_str), Some("tool_0499"));
}
