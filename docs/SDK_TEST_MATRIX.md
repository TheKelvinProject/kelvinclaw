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
  - privileged capability policy gates (`fs_read`, `fs_write`, `network_egress`, `command_execution`)
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
- `crates/kelvin-core/tests/sdk_owasp_top10_ai_2025.rs` OWASP Top 10 AI stress suite
- `crates/kelvin-core/tests/sdk_nist_ai_rmf_1_0.rs` NIST AI RMF 1.0 suite
- `docs/SDK_OWASP_TOP10_AI_2025.md` category-to-test mapping
- `docs/SDK_NIST_AI_RMF_1_0.md` function-to-test mapping
