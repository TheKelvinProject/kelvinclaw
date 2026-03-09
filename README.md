# kelvinclaw

KelvinClaw is a Secure, Stable, and Modular Runtime for Agentic Workflows.
It focuses on predictable runtime behavior, policy-driven extension loading, and a maintainable SDK surface for plugin developers.

SDK name: **Kelvin Core**.

What this project includes:

- control plane (`kelvin` root + brain): policy, orchestration, lifecycle
- data plane (`kelvin-memory-controller`): RPC memory operations with security checks
- SDK (`Kelvin Core`): stable interfaces for plugins, tools, and runtime integration
- plugin system: signed package install/verification and policy-based capability enforcement

For end users, plugins are installed as packages and executed by Kelvin. They do not need to compile the Rust workspace.

## Getting Started

Choose the onboarding path for your experience level:

- [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md)

Canonical quick start commands:

```bash
scripts/quickstart.sh --mode local
scripts/quickstart.sh --mode docker
```

Verify a specific path:

```bash
scripts/verify-onboarding.sh --track beginner
scripts/verify-onboarding.sh --track rust
scripts/verify-onboarding.sh --track wasm
scripts/verify-onboarding.sh --track daily
```

## Repository Layout

- `apps/kelvin-host`: thin trusted host executable
- `apps/kelvin-gateway`: secure WebSocket control-plane gateway
- `crates/*`: core contracts, runtime, SDK, memory API/client/controller, and execution engine
- first-party plugin distribution repo: `agentichighway/kelvinclaw-plugins`
- `examples/`: sample source crates for developers

## Architecture

See:

- [OVERVIEW.md](OVERVIEW.md)
- [docs/architecture.md](docs/architecture.md)
- [docs/gateway-protocol.md](docs/gateway-protocol.md)
- [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md)
- [docs/kelvin-gap-analysis.md](docs/kelvin-gap-analysis.md)
- [docs/KELVIN_CORE_SDK.md](docs/KELVIN_CORE_SDK.md)
- [docs/SDK_TEST_MATRIX.md](docs/SDK_TEST_MATRIX.md)
- [docs/SDK_OWASP_TOP10_AI_2025.md](docs/SDK_OWASP_TOP10_AI_2025.md)
- [docs/SDK_NIST_AI_RMF_1_0.md](docs/SDK_NIST_AI_RMF_1_0.md)
- [docs/PLUGIN_INSTALL_FLOW.md](docs/PLUGIN_INSTALL_FLOW.md)
- [docs/ROOT_VS_SDK.md](docs/ROOT_VS_SDK.md)
- [docs/CORE_ADMISSION_POLICY.md](docs/CORE_ADMISSION_POLICY.md)
- [docs/SDK_PRINCIPLES.md](docs/SDK_PRINCIPLES.md)
- [docs/trusted-executive-wasm.md](docs/trusted-executive-wasm.md)
- [docs/RUST_DEVELOPER_QUICKSTART.md](docs/RUST_DEVELOPER_QUICKSTART.md)
- [docs/memory-control-data-plane.md](docs/memory-control-data-plane.md)
- [docs/memory-rpc-contract.md](docs/memory-rpc-contract.md)
- [docs/memory-module-sdk.md](docs/memory-module-sdk.md)
- [docs/memory-controller-deployment-profiles.md](docs/memory-controller-deployment-profiles.md)
- [docs/model-plugin-abi.md](docs/model-plugin-abi.md)
- [docs/channel-plugin-abi.md](docs/channel-plugin-abi.md)
- [docs/openai-plugin-install-and-run.md](docs/openai-plugin-install-and-run.md)
- [docs/runtime-container-first-run.md](docs/runtime-container-first-run.md)
- [docs/plugin-index-schema.md](docs/plugin-index-schema.md)
- [docs/toolpack-sdk-plugins.md](docs/toolpack-sdk-plugins.md)
- [docs/plugin-author-kit.md](docs/plugin-author-kit.md)
- [docs/plugin-quality-tiers.md](docs/plugin-quality-tiers.md)
- [docs/plugin-trust-operations.md](docs/plugin-trust-operations.md)
- [docs/agents-tradeoffs.md](docs/agents-tradeoffs.md)
- [docs/compatibility-contracts.md](docs/compatibility-contracts.md)

Workspace crates:

- `crates/kelvin-core`: contracts and shared types
- `crates/kelvin-memory-api`: protobuf and gRPC service contracts
- `crates/kelvin-memory-client`: root-side RPC adapter implementing `MemorySearchManager`
- `crates/kelvin-memory-controller`: memory data plane gRPC server + WASM execution policy
- `crates/kelvin-memory-module-sdk`: memory module ABI helpers and WIT contract
- `crates/kelvin-memory`: in-process memory backends used by local/test compositions
- `crates/kelvin-brain`: agent loop orchestration
- `crates/kelvin-wasm`: trusted native executive for untrusted WASM skills

Apps:

- `apps/kelvin-host`: thin host executable for Kelvin SDK
- `apps/kelvin-gateway`: WebSocket gateway over SDK runtime

## Interface-First Design

Main traits:

- `Brain`
- `MemorySearchManager`
- `ModelProvider`
- `SessionStore`
- `Tool` / `ToolRegistry`
- `EventSink`
- `PluginFactory` / `PluginRegistry` (Kelvin Core SDK)
- `CoreRuntime` / `RunRegistry` (core lifecycle state machine)

Everything in the runtime is composed with trait objects so concrete implementations can be swapped.

## SDK Runtime Integration

The runtime integrates through the Kelvin Core SDK path:

- `WasmSkillPlugin` (plugin manifest + tool factory)
- `InMemoryPluginRegistry` (policy-gated registration)
- `SdkToolRegistry` (validated tool projection for runtime wiring)
- `SdkModelProviderRegistry` (validated model-provider projection)
- `kelvin_cli` (CLI plugin executed before each run)
- `kelvin.openai` (first-party OpenAI model plugin, optional)
- Kelvin Core tool-pack plugins (`fs_safe_read`, `fs_safe_write`, `web_fetch_safe`, `schedule_cron`, `session_tools`)

## Trusted Executive + Untrusted Skills

Kelvin now supports the split model:

- trusted native Rust host (`kelvin-wasm`) with system keys
- untrusted WASM skills loaded at runtime
- explicit host ABIs (`claw::*` for tools, `kelvin_model_host_v1` for model providers)
- sandbox policy gates that deny disallowed capabilities at module instantiation

Key types in `kelvin-wasm`:

- `WasmSkillHost`
- `SandboxPolicy`
- `ClawCall`
- `SandboxPreset`

Run a `.wasm` skill with the native executive:

```bash
cargo run -p kelvin-wasm --bin kelvin-wasm-runner -- --wasm path/to/skill.wasm --policy-preset locked_down
```

Model-provider ABI reference:

- [docs/model-plugin-abi.md](docs/model-plugin-abi.md)

## Memory Backend Swapping

`kelvin-memory::MemoryFactory` supports:

- `Markdown`
- `InMemoryVector`
- `InMemoryWithMarkdownFallback`

The fallback manager mimics KelvinClaw's primary->fallback behavior.

## CLI Example

```bash
scripts/install-kelvin-cli-plugin.sh
KELVIN_PLUGIN_HOME=.kelvin/plugins \
KELVIN_TRUST_POLICY_PATH=.kelvin/trusted_publishers.json \
CARGO_TARGET_DIR=target/try-kelvin-cli cargo run -p kelvin-host -- --prompt "hello" --workspace /path/to/workspace --memory fallback
```

OpenAI provider path:

```bash
scripts/install-kelvin-openai-plugin.sh
OPENAI_API_KEY=<your_key> \
KELVIN_PLUGIN_HOME=.kelvin/plugins \
KELVIN_TRUST_POLICY_PATH=.kelvin/trusted_publishers.json \
CARGO_TARGET_DIR=target/try-kelvin-cli cargo run -p kelvin-host -- --prompt "hello" --model-provider kelvin.openai --workspace /path/to/workspace --memory fallback
```

The CLI executable is only a thin launcher. Runtime behavior is composed in `kelvin-sdk`, and
the CLI path executes through an installed plugin (`kelvin_cli`) loaded through the
same secure installed-plugin path as third-party plugins.

Quick run:

```bash
scripts/try-kelvin.sh "hello"
```

Interactive mode:

```bash
cargo run -p kelvin-host -- --interactive --workspace /path/to/workspace --state-dir /path/to/workspace/.kelvin/state
```

## Gateway Example

Run the gateway with connect-token auth:

```bash
KELVIN_GATEWAY_TOKEN=change-me \
CARGO_TARGET_DIR=target/try-kelvin-gateway cargo run -p kelvin-gateway -- \
  --bind 127.0.0.1:18789 \
  --workspace /path/to/workspace
```

Methods available over the socket:

- `connect`
- `health`
- `agent` / `run.submit`
- `agent.wait` / `run.wait`
- `agent.state` / `run.state`
- `agent.outcome` / `run.outcome`
- `channel.telegram.ingest`
- `channel.telegram.pair.approve`
- `channel.telegram.status`
- `channel.slack.ingest`
- `channel.slack.status`
- `channel.discord.ingest`
- `channel.discord.status`
- `channel.route.inspect`

Operational scripts:

- `scripts/kelvin-gateway-daemon.sh start|stop|status|logs|health`
- `scripts/kelvin-local-profile.sh start|stop|status|doctor`
- `scripts/quickstart.sh --mode local|docker`
- `scripts/kelvin-doctor.sh`
- `scripts/kelvin-webchat.sh [port]`

`kelvin-doctor` and gateway `--doctor` output machine-readable checks with remediation hints.

## Runtime Container (No Rust Toolchain Required)

For end users, run the minimal runtime container and complete first-time setup interactively:

```bash
scripts/run-runtime-container.sh
```

This opens a setup wizard in-container, installs required plugins from the remote plugin index,
and prepares a persistent runtime home under `.kelvin/`.

After setup:

```bash
kelvin-host --prompt "What is KelvinClaw?" --timeout-ms 3000
```

Reference docs:

- [docs/runtime-container-first-run.md](docs/runtime-container-first-run.md)
- [docs/plugin-index-schema.md](docs/plugin-index-schema.md)

Tool-trigger pattern for the default model provider:

```text
[[tool:time]]
[[tool:hello_tool {"foo":"bar"}]]
```

## Remote Build and Test (Optional)

Remote testing is optional. Public clones can run local Docker tests without any private host setup.

Privacy-conscious remote setup:

```bash
cp .env.example .env
$EDITOR .env
scripts/remote-test.sh --docker
```

Additional variants:

```bash
REMOTE_TEST_HOST=your-user@your-host scripts/remote-test.sh
REMOTE_TEST_REMOTE_DIR=~/work/kelvinclaw scripts/remote-test.sh --native
scripts/remote-test.sh --docker
scripts/remote-test.sh --host your-user@your-host --cargo-args '-- --nocapture'
```

Notes:

- `.env` and `.env.local` are gitignored; keep personal hosts/IPs there only.
- `scripts/remote-test.sh` reads `REMOTE_TEST_HOST`, `REMOTE_TEST_REMOTE_DIR`, and `REMOTE_TEST_DOCKER_IMAGE` from `.env`/`.env.local`.
- `.env` files are parsed as key/value data and are not executed as shell code.

## Plugin Install (No Build Required)

Install Kelvin's first-party CLI plugin package:

```bash
scripts/install-kelvin-cli-plugin.sh
```

Install optional browser automation plugin profile:

```bash
scripts/install-kelvin-browser-plugin.sh
```

Default index:

- `https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json`

Install a prebuilt plugin package:

```bash
scripts/plugin-install.sh --package ./dist/acme.echo-1.0.0.tar.gz
```

List installed plugins:

```bash
scripts/plugin-list.sh
scripts/plugin-list.sh --json
```

Uninstall plugin:

```bash
scripts/plugin-uninstall.sh --id acme.echo --version 1.0.0
scripts/plugin-uninstall.sh --id acme.echo --purge
```

Run installer tests:

```bash
scripts/test-plugin-install.sh
scripts/test-cli-plugin-integration.sh
```

Plugin discovery:

```bash
scripts/plugin-discovery.sh
scripts/plugin-discovery.sh --plugin kelvin.cli
```

## Installed Plugin Runtime (Secure Loader)

`kelvin-brain` can load installed SDK plugin packages and project them into runtime tool/model providers with policy enforcement:

- signed manifest verification (`plugin.sig`, Ed25519 trusted publishers)
- manifest integrity validation (`entrypoint_sha256`)
- capability scopes (`fs_read_paths`, `network_allow_hosts`)
- operational controls (timeout, retries, rate limit, circuit breaker)
- runtime kind checks (`wasm_tool_v1`, `wasm_model_v1`)
- model-plugin import allowlist checks (`kelvin_model_host_v1` imports only)

Source: `crates/kelvin-brain/src/installed_plugins.rs`

Default boot helpers:

- `load_installed_plugins_default(core_version, security_policy)`
- `load_installed_tool_plugins_default(core_version, security_policy)`
- `default_plugin_home()`
- `default_trust_policy_path()`

Default paths:

- plugin home: `~/.kelvinclaw/plugins` (or `KELVIN_PLUGIN_HOME`)
- trust policy: `~/.kelvinclaw/trusted_publishers.json` (or `KELVIN_TRUST_POLICY_PATH`)

## Publisher Signing

Sign a package manifest and generate `plugin.sig`:

```bash
scripts/plugin-sign.sh \
  --manifest ~/.kelvinclaw/plugins/acme.echo/1.0.0/plugin.json \
  --private-key ~/.kelvinclaw/keys/acme-ed25519-private.pem \
  --publisher-id acme \
  --trust-policy-out ./trusted_publishers.acme.json
```

Trust policy operations:

```bash
scripts/plugin-trust.sh show
scripts/plugin-trust.sh rotate-key --publisher acme --public-key <base64>
scripts/plugin-trust.sh revoke --publisher acme
scripts/plugin-trust.sh pin --plugin acme.echo --publisher acme
```

Plugin author workflow:

```bash
export PATH="$PWD/scripts:$PATH"
kelvin plugin new --id acme.echo --name "Acme Echo" --runtime wasm_tool_v1
kelvin plugin test --manifest ./plugin-acme.echo/plugin.json
kelvin plugin pack --manifest ./plugin-acme.echo/plugin.json
kelvin plugin verify --package ./plugin-acme.echo/dist/acme.echo-0.1.0.tar.gz
```

Trust policy template:

- `trusted_publishers.example.json`

Host boot behavior:

- `apps/kelvin-host` calls `kelvin_sdk::run_with_sdk(...)` only.
- `kelvin-sdk` requires installed `kelvin_cli` and auto-loads installed SDK plugins with `load_installed_plugins_default(...)`.

## Local Test

```bash
cargo test --workspace
```

SDK certification lane:

```bash
scripts/test-sdk.sh
```

Docker:

```bash
scripts/test-docker.sh
```

Clean rebuild from zero (recommended before final pushes):

```bash
scripts/test-docker.sh --final
```

Build the sample Rust WASM skill:

```bash
cargo build --target wasm32-unknown-unknown --manifest-path examples/echo-wasm-skill/Cargo.toml
```

Run the sample skill:

```bash
cargo run -p kelvin-wasm --bin kelvin-wasm-runner -- --wasm examples/echo-wasm-skill/target/wasm32-unknown-unknown/debug/echo_wasm_skill.wasm
```
