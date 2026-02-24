# Root vs SDK Trust Model

KelvinClaw intentionally supports two extension lanes.

## 1. Root Lane (Unsafe Power)

Root integrations are direct code-level integrations with Kelvin internals.

- target users: core maintainers, advanced operators
- trust level: trusted code only
- security model: no SDK admission guardrails by default
- stability goal: reliable behavior, but internal APIs may evolve faster than SDK contracts

Root lane is open by design, but it is not a security boundary.

## 2. SDK Lane (Public Safety Contract)

SDK integrations use Kelvin Core plugin contracts and policy checks.

- target users: plugin authors and general integrators
- trust level: untrusted or semi-trusted code
- security model: capability declarations + policy-gated admission
- stability goal: semver compatibility and explicit contract guarantees

SDK lane is the recommended default for ecosystem extensions.

## 3. Policy

- If code must be installable by unknown users, it belongs in SDK lane.
- If code requires unrestricted internal access, it belongs in root lane and must be clearly documented as trusted-only.
- Security claims apply only to SDK lane unless explicitly stated otherwise.

## 4. Validation

Run SDK certification checks with:

```bash
scripts/test-sdk.sh
```

This script runs the SDK-focused test suite that verifies admission controls, projection safety, determinism, and concurrency behavior.
