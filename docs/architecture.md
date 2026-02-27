# KelvinClaw Architecture

## Purpose

KelvinClaw is a Rust, interface-first agent runtime that mirrors KelvinClaw's core architecture patterns:

- run acceptance + async completion (`agent` / `agent.wait` style)
- per-session serialized execution lanes
- lifecycle + assistant + tool event streams
- pluggable memory backend with fallback behavior
- strict trait boundaries so implementations are swappable

The intent is to keep the "brains" (orchestration and contracts) stable while enabling plug-and-play implementations for memory, models, tools, sessions, and delivery surfaces.

## Design Principles

1. Contracts first: behavior is defined by traits in `kelvin-core`.
2. Composition over inheritance: runtime behavior is assembled via dependency injection.
3. Deterministic orchestration: session-lane serialization avoids race conditions.
4. Failure containment: optional fallback managers prevent hard failures when a primary backend is unavailable.
5. Small surface area: each crate has a focused responsibility and clear boundaries.
6. Minimal core: provider/runtime specifics belong outside core contracts.

## Governance Documents

- [docs/KELVIN_CORE_SDK.md](docs/KELVIN_CORE_SDK.md)
- [docs/CORE_ADMISSION_POLICY.md](docs/CORE_ADMISSION_POLICY.md)
- [docs/SDK_PRINCIPLES.md](docs/SDK_PRINCIPLES.md)
- [docs/trusted-executive-wasm.md](docs/trusted-executive-wasm.md)
- [docs/memory-control-data-plane.md](docs/memory-control-data-plane.md)
- [docs/memory-rpc-contract.md](docs/memory-rpc-contract.md)
- [docs/memory-module-sdk.md](docs/memory-module-sdk.md)
- [docs/memory-controller-deployment-profiles.md](docs/memory-controller-deployment-profiles.md)

Use these as merge criteria when deciding whether logic belongs in core or in extensions.

## Workspace Topology

- `crates/kelvin-core`: domain models and interfaces.
- `crates/kelvin-memory-api`: protobuf schema + generated gRPC contracts.
- `crates/kelvin-memory-client`: RPC adapter implementing `MemorySearchManager`.
- `crates/kelvin-memory-controller`: memory data plane gRPC service + WASM sandbox runtime.
- `crates/kelvin-memory-module-sdk`: module ABI constants and WIT contracts.
- `crates/kelvin-memory`: transitional in-process memory backends (deprecated for root composition).
- `crates/kelvin-brain`: agent loop orchestration implementation.
- `crates/kelvin-wasm`: trusted host runtime for loading untrusted WASM skills.
- `apps/kelvin-host`: thin SDK host executable.

## Core Interfaces

Defined in `kelvin-core`:

- `Brain`
  - single-run orchestration boundary
- `MemorySearchManager`
  - `search`, `read_file`, `status`, `sync`, probe methods
- `ModelProvider`
  - model inference boundary
- `Tool` and `ToolRegistry`
  - tool invocation and discovery
- `PluginFactory` and `PluginRegistry`
  - plugin declaration, compatibility checks, and registration
- `SdkToolRegistry`
  - validated projection from plugin metadata into runtime `ToolRegistry`
- `CoreRuntime` and `RunRegistry`
  - run lifecycle (`accepted -> running -> completed|failed`) and wait semantics
- `SessionStore`
  - transcript/session persistence boundary
- `EventSink`
  - stream/event emission boundary

These traits are the architecture's stable API.

## Component Responsibilities

### Brain (`kelvin-brain`)

`KelvinBrain` orchestrates one run end-to-end:

1. Validate request.
2. Emit lifecycle start.
3. Ensure session record and persist user prompt.
4. Assemble context (history + memory recall).
5. Invoke model provider.
6. Emit assistant event(s).
7. Execute tool calls and emit tool events.
8. Persist assistant/tool transcript entries.
9. Emit lifecycle end or error.

### Runtime (`kelvin-core`)

`CoreRuntime` provides asynchronous run handling:

- `submit` returns immediate acceptance metadata.
- run executes in a background task.
- run state is persisted in `RunRegistry`.
- caller can inspect state and `wait` for completion with timeout.

`LaneScheduler` ensures per-session serialization.

### Memory Planes (`kelvin-memory-*`)

Control/Data split:

- Root (control plane) calls memory over gRPC through `RpcMemoryManager`.
- Memory Controller (data plane) re-validates JWT delegation claims and executes WASM memory modules.
- Module access is bounded by manifest capabilities, delegation claims, and enabled provider features.

### Transitional In-Proc Memory (`kelvin-memory`)

Backends:

- `MarkdownMemoryManager`
  - source-of-truth Markdown files (`MEMORY.md`, `memory/**/*.md`)
  - scoped reads and graceful missing-file behavior
- `InMemoryVectorMemoryManager`
  - in-memory token-overlap retrieval backend
- `FallbackMemoryManager`
  - delegates to primary; on failure, flips to fallback backend

Selection:

- `MemoryFactory` builds backend by `MemoryBackendKind` for migration/fallback only.

### WASM Executive (`kelvin-wasm`)

`WasmSkillHost` executes untrusted WebAssembly modules with explicit capability boundaries:

- exports expected from skill modules: `run() -> i32`
- host ABI imports exposed under `claw::*` (for example `send_message`, `move_servo`)
- `SandboxPolicy` controls which privileged imports are linked
- denied capabilities fail module instantiation before skill execution
- module size and fuel budget limits are enforced for DoS resistance

`kelvin-wasm-runner` provides a minimal host CLI for executing skill binaries with policy presets.

### Installed Plugin Runtime (`kelvin-brain`)

`installed_plugins` provides a secure loader for prebuilt SDK plugin packages under `~/.kelvinclaw/plugins` (or configured plugin home):

- runtime loader for installed `wasm_tool_v1` plugins
- execution bridge via SDK (`InMemoryPluginRegistry` -> `SdkToolRegistry` -> runtime `ToolRegistry`)
- supply-chain trust verification (`plugin.sig` Ed25519 against trusted publisher keys)
- scoped capability enforcement (`capability_scopes.fs_read_paths`, `capability_scopes.network_allow_hosts`)
- operational controls (`timeout_ms`, `max_retries`, rate limit, circuit breaker)

This keeps plugin execution out of Kelvin root code compilation while preserving host-side policy enforcement.

First-party CLI plugin path:

- installable first-party plugin artifacts are hosted in `TheKelvinProject/kelvinclaw-plugins`.
- `apps/kelvin-host` calls `kelvin-sdk`, which requires installed tool `kelvin_cli` before each run.

## Execution Flow

### High-Level

1. `apps/kelvin-host` parses input and calls `kelvin_sdk::run_with_sdk(...)`.
2. SDK loads installed plugins and requires tool `kelvin_cli`.
3. SDK runs `kelvin_cli` preflight, then constructs `CoreRuntime`.
4. `CoreRuntime` submits run state and executes `KelvinBrain`.
5. Events stream through `EventSink`.
6. Run completion/failure is stored in `RunRegistry`.
7. Caller waits for final status/outcome.

### Event Model

`AgentEventData` stream types:

- `lifecycle` (`start | end | error`)
- `assistant` (delta/final chunks)
- `tool` (`start | end | error`)

This aligns with KelvinClaw-style stream channels while remaining transport-agnostic.

## Extensibility and Swap Points

### Memory

Swap backend by changing one composition value (`MemoryBackendKind`) without touching orchestration logic.

### Models

Replace `Arc<dyn ModelProvider>` to support different provider implementations.

### Tools

Register tools through `ToolRegistry`; `SdkToolRegistry` can compose this directly from SDK plugin registrations with fail-fast validation.

### Sessions

Swap `SessionStore` for file/db/remote persistence.

### Events

Swap `EventSink` for stdout, in-memory capture, websocket bridge, etc.

## Failure and Timeout Semantics

- brain-level timeout can fail a run with `KelvinError::Timeout`
- runtime wait timeout returns `WaitStatus::Timeout` without forcing run cancellation
- fallback memory manager degrades gracefully when primary backend fails

## Testing Strategy

Current tests validate architecture behavior over implementation details:

- session-lane serialization for concurrent runs
- run wait timeout semantics
- result completion retrieval
- memory backend swap behavior
- memory fallback behavior
- graceful missing memory file reads

## Current Scope

Implemented:

- trait-oriented architecture and crate boundaries
- KelvinClaw-style run/event/memory seams
- swappable backends and adapters
- remote test workflow
- dynamic installed plugin loading with trust/scoping/operational guardrails
- websocket gateway control plane (`apps/kelvin-gateway`) with connect/auth/idempotent run submission
- SDK model failover chains with bounded retries and fail-closed non-retryable errors
- multi-channel gateway lane (`telegram`, `slack`, `discord`) with channel-specific policy controls
- deterministic channel routing rules by channel/account/workspace/session
- channel conformance tests (ordering/idempotency/auth mismatch/flood handling)
- optional WASM channel ingress policy ABI (`kelvin_channel_host_v1`)

Not yet implemented:

- additional channel integrations beyond telegram/slack/discord and richer operator UI surfaces
- provider credential profile routing trees for non-channel model lanes
- full compaction/retry pipelines

## Operational Notes

- Local remote-test helper: `scripts/remote-test.sh`
- Remote host can be provided by `--host` or `REMOTE_TEST_HOST` (including `.env` convenience loading)
- Build/test can run natively on ARM64 EC2 or inside Docker mode
