# OpenClaw Gap Analysis -> KelvinClaw Refactor

## Objective

Refactor KelvinClaw into an interface-first Rust architecture that mirrors OpenClaw's runtime "brain" structure and backend abstraction points.

## Canonical OpenClaw Behaviors Mapped

### 1. Agent Loop and Stream Events

OpenClaw signals:

- serialized runs per session lane
- lifecycle start/end/error stream
- assistant delta stream
- tool stream

KelvinClaw mapping:

- `kelvin-brain::OpenClawBrain`
- `kelvin-core::AgentEventData`
- `archive/kelvin-runtime::LaneScheduler` (archived reference implementation)
- `archive/kelvin-runtime::AgentRuntime` (archived reference implementation)

### 2. Memory Manager Contract

OpenClaw signals:

- `search`, `readFile`, `status`, `sync`, probe methods
- swappable backend (builtin vs qmd)
- fallback to builtin when primary backend fails

KelvinClaw mapping:

- `kelvin-core::MemorySearchManager`
- `kelvin-memory::MarkdownMemoryManager`
- `kelvin-memory::InMemoryVectorMemoryManager`
- `kelvin-memory::FallbackMemoryManager`
- `kelvin-memory::MemoryFactory`

### 3. Run Registry and Wait Semantics

OpenClaw signals:

- immediate `accepted` response
- async run completion
- wait with timeout

KelvinClaw mapping:

- `kelvin-core::RunRegistry`
- `kelvin-core::CoreRuntime::submit`
- `kelvin-core::CoreRuntime::wait`
- `kelvin-core::CoreRuntime::wait_for_outcome`

## Interface Inventory

Implemented in `kelvin-core`:

- `Brain`
- `MemorySearchManager`
- `ModelProvider`
- `SessionStore`
- `EventSink`
- `Tool`
- `ToolRegistry`
- `PluginFactory`
- `PluginRegistry`
- `CoreRuntime`
- `RunRegistry`

## Plug-and-Play Examples

- Swap memory backend with one line in composition code:
  - `MemoryBackendKind::Markdown`
  - `MemoryBackendKind::InMemoryVector`
  - `MemoryBackendKind::InMemoryWithMarkdownFallback`
- Swap model provider by replacing `Arc<dyn ModelProvider>`.
- Swap session persistence by replacing `Arc<dyn SessionStore>`.
- Swap event emission target by replacing `Arc<dyn EventSink>`.

## Tests Added

`crates/kelvin-core/src/runtime.rs`:

- serializes runs in same session lane
- wait timeout behavior
- completed outcome retrieval

## Remaining Work for Full OpenClaw Parity

- gateway WS protocol and frame validation
- model-specific auth/failover logic
- compaction and retry orchestration
- plugin loader/runtime (currently direct composition)
- richer memory retrieval (embeddings/vector DB/QMD sidecar)
