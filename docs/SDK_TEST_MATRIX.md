# SDK Test Matrix

This matrix keeps Kelvin Core SDK verification focused on security, stability, and deterministic behavior.

## CRUD (Baseline)

- `create`: plugin registration acceptance/rejection under policy
- `read`: lookup (`get`) and manifest inventory (`manifests`)
- `update`: intentionally unsupported; duplicate id registration must fail
- `delete`: intentionally unsupported in current minimal registry

## Additional Required Abstractions

- `admission control`
  - semver validation
  - API major compatibility
  - privileged capability policy gates (`network_egress`, `fs_write`, `command_execution`)
  - experimental plugin gating
- `projection safety`
  - plugin capability declaration must match implementation
  - duplicate tool names fail fast
  - metadata-only plugins are ignored by tool projection
- `determinism`
  - stable tool name ordering from `SdkToolRegistry::names`
- `concurrency safety`
  - concurrent duplicate registration allows exactly one success
- `fail-closed errors`
  - invalid core version input is rejected
  - unknown plugin lookup returns `None`

## Implemented Suites

- `crates/kelvin-core/src/sdk.rs` unit tests
- `crates/kelvin-core/tests/sdk_security_stability.rs` integration tests
