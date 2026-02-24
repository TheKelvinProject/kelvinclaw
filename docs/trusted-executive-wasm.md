# Trusted Executive + WASM Skills

KelvinClaw now supports a split architecture:

- Trusted Foundation: native Rust host runtime in `crates/kelvin-wasm`
- Untrusted Extensions: skill modules compiled to `.wasm`

## Native Rust Executive

`WasmSkillHost` is the trusted host:

- loads wasm modules
- links only approved `claw::*` imports
- executes exported `run()` entrypoint
- records host calls (`ClawCall`) for observability

This layer holds all privileged capability decisions.

## WASM Skill Interface (`ClawCall`)

Host ABI module: `claw`
ABI version: `1.0.0`

Supported imports:

- `send_message(i32) -> i32` (always available)
- `move_servo(i32, i32) -> i32` (policy-gated)
- `fs_read(i32) -> i32` (policy-gated)
- `network_send(i32) -> i32` (policy-gated)

Skills that import capabilities not granted by policy are rejected at instantiation time.

## Sandbox Policy

`SandboxPolicy` defines what a skill may request:

- `allow_move_servo`
- `allow_fs_read`
- `allow_network_send`

Default is locked-down (`false` for all privileged capabilities).

Built-in presets:

- `locked_down`: no privileged capabilities
- `dev_local`: enables `fs_read` only
- `hardware_control`: enables `move_servo` only

Additional hardening knobs:

- `max_module_bytes`: hard limit on wasm binary size before compilation
- `fuel_budget`: execution fuel cap to terminate runaway guest loops

## Fort Knox Behavior

If a downloaded skill asks for filesystem/network capability without policy approval:

1. the import is not linked by the trusted executive
2. module instantiation fails
3. host remains intact and can discard the skill

## Validation

`kelvin-wasm` tests verify:

- allowed host calls execute
- denied filesystem calls are rejected
- policy-enabled filesystem calls succeed
- modules requiring WASI imports are rejected by default
- unknown ABI imports are rejected
- oversized modules are rejected before compile
- infinite loops are interrupted by fuel exhaustion
