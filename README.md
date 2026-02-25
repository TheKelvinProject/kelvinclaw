# kelvinclaw

Rust re-architecture of KelvinClaw-style "brain" + memory/runtime seams with strict interfaces for plug-and-play implementations.

SDK name: **Kelvin Core**.

## Architecture

See:

- [OVERVIEW.md](OVERVIEW.md)
- [docs/architecture.md](docs/architecture.md)
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

Workspace crates:

- `crates/kelvin-core`: contracts and shared types
- `crates/kelvin-memory`: memory backends + fallback manager
- `crates/kelvin-brain`: agent loop orchestration
- `crates/kelvin-wasm`: trusted native executive for untrusted WASM skills

Archived crates:

- `archive/kelvin-runtime`: archived run registry, lane scheduler, adapters
- `archive/kelvin-cli`: archived executable wiring (not in workspace members)

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

## SDK Dogfooding

The MVP secure skill loop is wired through the Kelvin Core SDK path:

- `WasmSkillPlugin` (plugin manifest + tool factory)
- `InMemoryPluginRegistry` (policy-gated registration)
- `SdkToolRegistry` (validated tool projection for runtime wiring)

## Trusted Executive + Untrusted Skills

Kelvin now supports the split model:

- trusted native Rust host (`kelvin-wasm`) with system keys
- untrusted WASM skills loaded at runtime
- explicit host ABI (`claw::*` imports) for what skills may request
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

## Memory Backend Swapping

`kelvin-memory::MemoryFactory` supports:

- `Markdown`
- `InMemoryVector`
- `InMemoryWithMarkdownFallback`

The fallback manager mimics KelvinClaw's primary->fallback behavior.

## CLI Example

```bash
cargo run --manifest-path archive/kelvin-cli/Cargo.toml -- --prompt "hello" --workspace /path/to/workspace --memory fallback
```

Tool-trigger pattern for the default model provider:

```text
[[tool:time]]
[[tool:hello_tool {"foo":"bar"}]]
```

## Optional: Remote Build/Test

Remote testing is optional. Public clones can run local Docker tests without any private host setup.

Privacy-conscious remote setup:

```bash
cp .env.example .env
$EDITOR .env
scripts/remote-test.sh --docker
```

Useful variants:

```bash
REMOTE_TEST_HOST=your-user@your-host scripts/remote-test.sh
REMOTE_TEST_REMOTE_DIR=~/work/kelvinclaw scripts/remote-test.sh --native
scripts/remote-test.sh --docker
scripts/remote-test.sh --host your-user@your-host --cargo-args '-- --nocapture'
```

Notes:

- `.env` and `.env.local` are gitignored; keep personal hosts/IPs there only.
- `scripts/remote-test.sh` reads only `REMOTE_TEST_HOST` and `REMOTE_TEST_REMOTE_DIR` from `.env`/`.env.local`.
- `.env` files are parsed as key/value data and are not executed as shell code.

## Plugin Install (No Build Required)

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
```

## Installed Plugin Runtime (Secure Loader)

`kelvin-brain` can load installed SDK plugin packages and project them into runtime tools with policy enforcement:

- signed manifest verification (`plugin.sig`, Ed25519 trusted publishers)
- manifest integrity validation (`entrypoint_sha256`)
- capability scopes (`fs_read_paths`, `network_allow_hosts`)
- operational controls (timeout, retries, rate limit, circuit breaker)

Source: `crates/kelvin-brain/src/installed_plugins.rs`

Default boot helpers:

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

Trust policy template:

- `trusted_publishers.example.json`

Archived CLI boot behavior:

- `archive/kelvin-cli` now auto-loads installed SDK plugins with `load_installed_tool_plugins_default(...)` and keeps built-in tools as fallback.

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
docker run --rm -v "$PWD:/work" -w /work rust:1.77 cargo test --workspace
```

Build the sample Rust WASM skill:

```bash
cargo build --target wasm32-unknown-unknown --manifest-path skills/echo-wasm-skill/Cargo.toml
```

Run the sample skill:

```bash
cargo run -p kelvin-wasm --bin kelvin-wasm-runner -- --wasm skills/echo-wasm-skill/target/wasm32-unknown-unknown/debug/echo_wasm_skill.wasm
```
