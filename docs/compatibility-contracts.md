# Kelvin Compatibility Contracts

This document defines the explicit compatibility anchors for KelvinClaw public integrations.

## Contract Anchors

- Kelvin Core SDK API: `kelvin_core::KELVIN_CORE_API_VERSION` (`1.0.0`)
- Gateway protocol: `GATEWAY_PROTOCOL_VERSION` (`1.0.0`)
- Memory RPC API package: `kelvin.memory.v1alpha1` (`v1alpha1`)
- WASM tool ABI: `claw` module ABI version `1.0.0`
- WASM model ABI: `kelvin_model_host_v1` ABI version `1.0.0`

## Rules

- Major versions are compatibility boundaries.
- Field and method removals/renames are disallowed within a major version.
- New fields/methods are additive and must preserve existing behavior.
- Unknown or invalid config values fail closed with typed errors.

## Verification Gates

Contract compatibility is enforced by tests in:

- `crates/kelvin-memory-api` descriptor contract tests
- `apps/kelvin-gateway` protocol contract tests
- `crates/kelvin-wasm` ABI constant/import allowlist tests
