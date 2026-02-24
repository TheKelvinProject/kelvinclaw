# kelvinclaw

Rust re-architecture of OpenClaw-style "brain" + memory/runtime seams with strict interfaces for plug-and-play implementations.

## Architecture

See:

- [OVERVIEW.md](OVERVIEW.md)
- [docs/openclaw-gap-analysis.md](docs/openclaw-gap-analysis.md)

Workspace crates:

- `crates/kelvin-core`: contracts and shared types
- `crates/kelvin-memory`: memory backends + fallback manager
- `crates/kelvin-brain`: agent loop orchestration
- `crates/kelvin-runtime`: run registry, lane scheduler, adapters
- `crates/kelvin-cli`: executable wiring

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
cargo run -p kelvin-cli -- --prompt "hello" --workspace /path/to/workspace --memory fallback
```

Tool-trigger pattern for the default model provider:

```text
[[tool:time]]
[[tool:hello_tool {"foo":"bar"}]]
```

## Note

Rust toolchain commands (`cargo`, `rustc`) were not available in the current execution environment, so compile/test commands could not be run here.
