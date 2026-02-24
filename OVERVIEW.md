# KelvinClaw Overview

This repository implements a Rust architecture that mirrors OpenClaw's core runtime seams:

- Gateway-style run model: `submit` + `wait` with a run registry.
- Serialized agent loop per session lane.
- Lifecycle/assistant/tool stream events.
- Pluggable memory manager with backend fallback.
- Trait-first wiring so implementations can be swapped without changing the brain.

## OpenClaw Sources Used

A literal `OVERVIEW.md` was not present in the cloned `openclaw` repository. This design is grounded in:

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
- `crates/kelvin-brain`: OpenClaw-style orchestration loop (`OpenClawBrain`).
- `archive/kelvin-runtime`: archived lane scheduler, run registry, adapters.
- `archive/kelvin-cli`: archived runnable composition layer (excluded from workspace members).

## Primary Interfaces (Plug-and-Play Boundaries)

From `kelvin-core`:

- `Brain`
- `MemorySearchManager`
- `ModelProvider`
- `SessionStore`
- `EventSink`
- `Tool` / `ToolRegistry`

You can replace any implementation as long as it satisfies these traits.

## Memory Architecture

The memory layer follows OpenClaw's contract style:

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

Factory routing:

- `MemoryFactory::build(..., MemoryBackendKind::...)`

## Brain Loop

`OpenClawBrain` follows these phases:

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

## Current Scope vs OpenClaw

This repo now mirrors OpenClaw's architecture and contracts, not full feature parity.

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
