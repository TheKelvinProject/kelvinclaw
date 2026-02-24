# kelvinclaw

Rust re-architecture of OpenClaw-style "brain" + memory/runtime seams with strict interfaces for plug-and-play implementations.

## Architecture

See:

- [OVERVIEW.md](OVERVIEW.md)
- [docs/architecture.md](docs/architecture.md)
- [docs/openclaw-gap-analysis.md](docs/openclaw-gap-analysis.md)

Workspace crates:

- `crates/kelvin-core`: contracts and shared types
- `crates/kelvin-memory`: memory backends + fallback manager
- `crates/kelvin-brain`: agent loop orchestration
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

Everything in the runtime is composed with trait objects so concrete implementations can be swapped.

## Memory Backend Swapping

`kelvin-memory::MemoryFactory` supports:

- `Markdown`
- `InMemoryVector`
- `InMemoryWithMarkdownFallback`

The fallback manager mimics OpenClaw's primary->fallback behavior.

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

## Note

Rust toolchain commands (`cargo`, `rustc`) were not available in the current execution environment, so compile/test commands could not be run here.
