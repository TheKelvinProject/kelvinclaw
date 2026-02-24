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

## Remote EC2 Build/Test

One-command sync + remote test runner:

```bash
scripts/remote-test.sh
```

Useful variants:

```bash
REMOTE_TEST_HOST=ec2-user@your-host scripts/remote-test.sh
scripts/remote-test.sh --docker
scripts/remote-test.sh --host ec2-user@your-host --cargo-args '-- --nocapture'
```

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
