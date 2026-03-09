# KelvinClaw Overview

This repository implements a Rust architecture that mirrors KelvinClaw's core runtime seams:

- Gateway-style run model: `submit` + `wait` with a run registry.
- Serialized agent loop per session lane.
- Lifecycle/assistant/tool stream events.
- Pluggable memory manager with backend fallback.
- Trait-first wiring so implementations can be swapped without changing the brain.

SDK naming: the extension contract layer is called **Kelvin Core**.

## KelvinClaw Sources Used

A literal `OVERVIEW.md` was not present in the cloned `KelvinClaw` repository. This design is grounded in:

- `docs/concepts/architecture.md`
- `docs/concepts/agent-loop.md`
- `docs/concepts/memory.md`
- `src/memory/types.ts`
- `src/memory/search-manager.ts`
- `src/memory/manager.ts`
- `src/agents/pi-embedded-runner/run.ts`
- `src/agents/pi-embedded-subscribe.ts`

## Rust Workspace Layout

- `crates/kelvin-core`: domain contracts and traits.
- `crates/kelvin-memory`: memory backends and fallback wrapper.
- `crates/kelvin-brain`: KelvinClaw-style orchestration loop (`KelvinBrain`).
- `crates/kelvin-wasm`: trusted native executive for untrusted WASM skill execution.
- `apps/kelvin-host`: active runnable composition layer for SDK orchestration.
- first-party installable plugin artifacts live in `agentichighway/kelvinclaw-plugins`.

## Minimal Core Governance

KelvinClaw now formalizes "small core, extensible ecosystem" rules:

- [docs/KELVIN_CORE_SDK.md](docs/KELVIN_CORE_SDK.md): 8-part Kelvin Core SDK implementation.
- [docs/CORE_ADMISSION_POLICY.md](docs/CORE_ADMISSION_POLICY.md): strict criteria for what can enter `kelvin-core`.
- [docs/SDK_PRINCIPLES.md](docs/SDK_PRINCIPLES.md): plugin/SDK expectations for stability, safety, and security.
- [docs/trusted-executive-wasm.md](docs/trusted-executive-wasm.md): trusted host + untrusted skill runtime model.

These documents are intended to keep core tiny while making extension surfaces predictable.

## Primary Interfaces (Plug-and-Play Boundaries)

From `kelvin-core`:

- `Brain`
- `MemorySearchManager`
- `ModelProvider`
- `SessionStore`
- `EventSink`
- `PluginFactory`
- `PluginRegistry`
- `CoreRuntime`
- `RunRegistry`
- `Tool` / `ToolRegistry`

You can replace any implementation as long as it satisfies these traits.

## Memory Architecture

The memory layer follows KelvinClaw's contract style:

- `search(query, opts)`
- `read_file(rel_path, from, lines)`
- `status()`
- `sync(...)`
- `probe_embedding_availability()`
- `probe_vector_availability()`

Implemented backends:

- `MarkdownMemoryManager` (workspace `MEMORY.md` + `memory/**/*.md`)
- `InMemoryVectorMemoryManager` (volatile token-overlap index)
- `FallbackMemoryManager` (primary failure -> fallback)

## WASM Skill Security

`kelvin-wasm` provides ABI-locked WASM skill execution with:

- explicit host ABI (`claw::*`) and import validation
- sandbox policy presets (`locked_down`, `dev_local`, `hardware_control`)
- module-size and fuel-budget limits for isolation and stability

Factory routing:

- `MemoryFactory::build(..., MemoryBackendKind::...)`

## Brain Loop

`KelvinBrain` follows these phases:

1. Validate request and emit lifecycle `start`.
2. Persist user input to session store.
3. Assemble history + memory recall.
4. Run model inference.
5. Emit assistant stream event(s).
6. Execute tool calls and emit tool start/end/error events.
7. Persist assistant/tool transcript.
8. Emit lifecycle `end` (or `error` on failure).

## Runtime Semantics

`AgentRuntime` provides:

- `submit(request)` -> immediate `run_id` acceptance.
- `wait(run_id, timeout_ms)` -> `ok | error | timeout`.
- `wait_for_outcome(...)` -> completed payloads or failure.

Scheduling:

- `LaneScheduler` serializes execution per `session_key`.
- Optional global lane lock is supported.

## Current Scope vs KelvinClaw

This repo now mirrors KelvinClaw's architecture and contracts, not full feature parity.

Included:

- architecture and interface parity for memory/brain/runtime seams
- pluggable backends
- stream event model
- run registry and queue semantics
- tests for serialization and wait behavior

Not included yet:

- WebSocket gateway protocol server
- provider-specific model adapters (OpenAI/Gemini/etc)
- full compaction/retry pipeline
- full plugin loading system
