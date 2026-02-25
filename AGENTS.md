# KelvinClaw Agent Instructions

This file defines default expectations for coding agents working in this repository.

## Scope

- Work from the repository root.
- Make targeted, minimal changes.
- Prefer interface-first changes that preserve SDK contracts.

## Priorities

1. Security

- Protect secrets and sensitive information.
- Avoid introducing vulnerabilities or attack vectors.
- Ensure safe defaults and fail-closed behavior.
- Avoid over-permissioning or excessive access.
- Avoid destructive operations without explicit intent.

2. Stability
3. Reliability
4. Simplicity
5. Size/maintainability

## Safety Rules

- Never commit secrets, keys, tokens, hostnames, or private IPs.
- Keep `.env` / local machine details out of commits.
- Avoid destructive git commands unless explicitly requested.
- Do not revert user-authored unrelated changes.

## Architectural Principles

- All Crates must be self-contained and not directly reference each other, except for the SDK which can reference all.
- The Core SDK and the Memory SDK should be the only interfaces to the crates from the outside.
- All WASM plugins must be loaded through the SDK and not directly from the root or other crates.
- All network access must be mediated through the SDK with explicit allowlists and not directly from the root or other crates.
- All configuration must be validated and fail closed on missing or invalid values, with clear error messages.
- Keep everything as simple as possible, but no simpler. Avoid unnecessary complexity or abstractions.
- Bear in mind at all times the OWASP Top 10, NIST CSF / AI, MITRE ATT&CK, ISO 42001, and other relevant security frameworks and best practices.
- Follow Rust best practices for safety, error handling, and code quality.
- Follow general software engineering best practices for testing, documentation, and maintainability.
- Prioritize security and stability over new features or optimizations.
- Always consider the potential impact of changes on users and the ecosystem.
- Communicate clearly and proactively about changes, especially breaking ones, with users and stakeholders.
- Continuously monitor and improve the security, stability, and reliability of the system over time.

## Build and Test

- Only run tests/builds relevant to the change being made, but ensure all tests pass before finalizing.
- Prefer Docker-based verification, remote server first if available, only fallback to local if needed.
- Standard SDK lane:
    - `scripts/test-sdk.sh`
- Targeted Rust lane:
    - `cargo test -p kelvin-core -p kelvin-wasm -p kelvin-brain -p kelvin-sdk --lib`
- Run formatting checks before finalizing:
    - `cargo fmt --all -- --check`
- Run linting checks before finalizing:
    - `cargo clippy --all -- -D warnings`
- Run security checks before finalizing:
    - `cargo audit`
- Run dependency checks before finalizing:
    - `cargo outdated`
- Run integration tests before finalizing:
    - `cargo test --workspace --tests`
- Run end-to-end tests before finalizing:
    - `scripts/test-e2e.sh`
- Run Docker-based tests before finalizing:
    - `scripts/test-docker.sh`

## Plugin Architecture Guardrails

- Keep model/tool plugins on the SDK path, not direct root coupling.
- Fail closed on missing or invalid plugin configuration.
- Enforce manifest capability/runtime parity and import allowlists.
- Keep network access host-mediated with explicit allowlists.

## Commit Discipline

- Keep commits scoped and atomic.
- Only stage files relevant to the requested change.
- Use clear commit messages that describe intent.
