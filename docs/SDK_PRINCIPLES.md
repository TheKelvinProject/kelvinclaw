# Kelvin Core SDK Principles

This document defines the plugin and extension contract for Kelvin Core. The goal is a lightweight core with powerful, swappable implementations.

## SDK Goals

- Simple: small interfaces with clear input/output contracts.
- Stable: semver-aware evolution with minimal breaking changes.
- Safe: explicit trust boundaries and least-privilege defaults.
- Secure: no hidden capability escalation through plugins.
- Observable: plugin actions are visible through structured events.

## Extension Surfaces

Kelvin Core exposes extension points through core traits:

- `ModelProvider`
- `MemorySearchManager`
- `Tool` and `ToolRegistry`
- `SessionStore`
- `EventSink`
- `Brain` (advanced replacement)

Each surface is swappable by dependency injection, not hard-coded wiring.

## Plugin Contract Expectations

Every implementation should:

1. Validate inputs and return explicit errors.
2. Avoid panics; failures should map to `KelvinError`.
3. Be deterministic for identical input where feasible.
4. Respect cancellation/timeouts from caller context.
5. Avoid side effects outside declared scope.
6. Emit meaningful telemetry through `EventSink` integration points.

## Security Requirements

Plugin code must follow least privilege:

- filesystem scope restrictions for file-reading tools/memory
- explicit allowlists for network egress where applicable
- strict parsing/validation of JSON tool arguments
- no credential logging
- sanitize user-provided paths and commands

If a plugin cannot meet these constraints, it should be marked experimental and disabled by default.

## Versioning and Compatibility

- `kelvin-core` is the compatibility anchor for plugin authors.
- Prefer additive API changes.
- Breaking trait changes require major version increments and migration notes.
- New optional capabilities should be introduced through new traits or capability flags, not by breaking existing contracts.

## Packaging Guidance

- Keep plugins in separate crates (or external repos) from core.
- Keep dependencies localized to the plugin crate.
- Ship clear README examples for wiring and configuration.
- Include test doubles/mocks to make plugin behavior easy to verify.

## Testing Baseline For Plugins

Recommended minimum:

- unit tests for happy path and failure path
- contract tests against core trait expectations
- integration test with `kelvin-brain` orchestration where relevant
- fuzz/property tests for untrusted input parsing when risk is high

## Operational Guardrails

- Default to disabled for high-risk capabilities (command execution, broad network access).
- Record plugin name/version in run metadata where possible.
- Expose health/status checks for long-running components.

These guardrails keep Kelvin Core "small core, big ecosystem" without compromising stability or trust.
