# Kelvin Core SDK

Kelvin Core is the extension SDK for KelvinClaw. It keeps the runtime minimal while allowing pluggable implementations behind stable interfaces.

## 1. SDK Identity

- Canonical SDK name: `Kelvin Core`
- Core SDK API version constant: `KELVIN_CORE_API_VERSION`
- Source: `crates/kelvin-core/src/sdk.rs`

## 2. Stable Contracts

Extension boundaries remain trait-first:

- `ModelProvider`
- `MemorySearchManager`
- `Tool`
- `SessionStore`
- `EventSink`
- `CoreRuntime` / `RunRegistry` for deterministic run lifecycle semantics

Source: `crates/kelvin-core/src/*.rs`

## 3. Plugin Manifest Schema

`PluginManifest` defines extension metadata:

- `id`, `name`, `version`, `api_version`
- `capabilities`
- compatibility bounds (`min_core_version`, `max_core_version`)
- `experimental` flag

Source: `crates/kelvin-core/src/sdk.rs`

## 4. Capability and Permission Model

`PluginCapability` captures required powers:

- interface capabilities (model/memory/tool/session/event)
- privileged capabilities (`network_egress`, `fs_write`, `command_execution`)

`PluginSecurityPolicy` controls what is allowed at registration time.

Source: `crates/kelvin-core/src/sdk.rs`

## 5. Compatibility Gate

`check_plugin_compatibility(...)` validates:

- manifest schema correctness
- API major-version compatibility
- core-version range matching
- security policy compliance

Source: `crates/kelvin-core/src/sdk.rs`

## 6. Registry and Composition

`PluginFactory` exposes concrete implementations without coupling core to vendor crates.

`PluginRegistry` and `InMemoryPluginRegistry` provide:

- plugin registration with compatibility checks
- lookup by plugin id
- manifest inventory

`SdkToolRegistry` provides:

- fail-fast projection from plugin metadata to runtime `ToolRegistry`
- duplicate-tool-name rejection
- capability/implementation consistency checks (`tool_provider` capability must match actual tool export)

Source: `crates/kelvin-core/src/sdk.rs`

## 7. Conformance Tests

Current SDK tests cover:

- manifest validation failures
- policy-based capability rejection
- compatibility acceptance
- registry registration/get/list
- duplicate registration rejection
- core-version range rejection
- `SdkToolRegistry` build success for registered tool plugins
- rejection of missing tool implementation when `tool_provider` is declared
- rejection of duplicate tool names across plugins

Source: `crates/kelvin-core/src/sdk.rs` (`#[cfg(test)]`)

## 8. Governance and Adoption

SDK operation is governed by:

- `docs/CORE_ADMISSION_POLICY.md`
- `docs/SDK_PRINCIPLES.md`

This keeps Kelvin small and stable while making plugin development predictable and safe.
