# Rust Developer Quickstart

This is the fastest path to try KelvinClaw from a fresh clone.
For beginner and WASM-author paths, see [docs/GETTING_STARTED.md](GETTING_STARTED.md).

## 1) Run Kelvin in one command

```bash
scripts/try-kelvin.sh "hello kelvin"
```

What this does:

- uses local `cargo` if installed
- otherwise falls back to Docker (`rust:1.93.1-bookworm` by default)
- installs/updates the first-party `kelvin_cli` WASM plugin package into `./.kelvin/plugins`
- runs `apps/kelvin-host` with a prompt

Expected output includes:

- cli plugin preflight (`kelvin_cli executed ...`)
- run accepted
- lifecycle events (`start` / `end`)
- assistant payload (echo provider for MVP)

## 2) Force local or Docker mode

```bash
KELVIN_TRY_MODE=local scripts/try-kelvin.sh "status check"
KELVIN_TRY_MODE=docker scripts/try-kelvin.sh "status check"
```

Optional timeout override:

```bash
KELVIN_TRY_TIMEOUT_MS=8000 scripts/try-kelvin.sh "longer timeout"
```

## 3) Validate security/stability suites

Track verification command:

```bash
scripts/verify-onboarding.sh --track rust
```

SDK suites:

```bash
scripts/test-sdk.sh
scripts/test-cli-plugin-integration.sh
scripts/test-docker.sh
```

Before final pushes:

```bash
scripts/test-docker.sh --final
```

Memory controller OWASP + NIST suites:

```bash
cargo test -p kelvin-memory-controller --test memory_controller_owasp_top10_ai_2025
cargo test -p kelvin-memory-controller --test memory_controller_nist_ai_rmf_1_0
```

## 4) Current MVP behavior

- The default demo path uses the built-in echo model provider.
- CLI flow is SDK-first and runs through a WASM plugin (`kelvin_cli`) before run execution.
- Memory/data-plane split exists and is tested.
- Plugin install path is prebuilt-package based (no recompiling root required).

For architecture details, see:

- `docs/architecture.md`
- `docs/memory-control-data-plane.md`
- `docs/PLUGIN_INSTALL_FLOW.md`
