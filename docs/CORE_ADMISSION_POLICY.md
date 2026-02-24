# KelvinCore Admission Policy

This document defines what is allowed into KelvinClaw core (`crates/kelvin-core`) and what must stay in extensions/adapters.

## Core Mission

Kelvin core exists to provide stable contracts and deterministic orchestration primitives for a minimal "Claw":

- run request/response types
- stream event types
- trait interfaces (`Brain`, `MemorySearchManager`, `ModelProvider`, `Tool`, `SessionStore`, `EventSink`)
- shared errors and compatibility-safe value objects

Core should remain small, readable, and hard to break.

The SDK identity for these contracts is **Kelvin Core** (see `docs/KELVIN_CORE_SDK.md`).

## Hard Admission Gate

A change can enter core only if all checks pass:

1. Required everywhere: every Kelvin deployment needs it, independent of provider/vendor/runtime.
2. Contract over implementation: it defines behavior boundaries, not a concrete backend.
3. Deterministic semantics: behavior is predictable and testable without external services.
4. Security baseline: no expanded privilege surface, no implicit filesystem/network access, no secret coupling.
5. Dependency discipline: no heavy or provider-specific dependency is introduced.
6. Version stability: the API can be maintained under semver without frequent breakage.

If one check fails, the change belongs in a plugin/adapter crate.

## What Must Stay Out Of Core

- provider-specific clients (LLM SDKs, cloud APIs, vector databases)
- transport implementations (websocket/http servers, gateway protocol wiring)
- persistence engines (sql, redis, object stores) beyond trait contracts
- business/product policy logic that can vary by deployment
- large utility layers that are not required for core contracts

## Dependency Policy

Core dependencies must be:

- broadly trusted and mature
- small in transitive footprint
- runtime-agnostic where possible

Any new dependency must include a short justification in the PR description:

- why core needs it
- what alternatives were rejected
- expected maintenance/security impact

## Security and Safety Rules

- No `unsafe` code in core.
- No direct secret loading or credential management in core.
- Validate untrusted input at boundary types where practical.
- Keep error messages useful but avoid leaking sensitive runtime details.

## Stability Contract

`kelvin-core` should be treated as the SDK compatibility anchor:

- breaking changes require a major version bump
- additive changes are preferred over mutating existing fields/traits
- deprecated fields/traits should include a transition window and migration notes

## Review Checklist

Before merging any core change, confirm:

- it passes all hard admission checks
- tests cover expected and error paths
- docs reflect any contract changes
- change does not force plugin authors into unnecessary rewrites
