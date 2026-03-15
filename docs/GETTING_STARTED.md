# Getting Started

KelvinClaw supports three onboarding tracks based on user experience level.
Each track has a verification command so the setup can be validated immediately.

## Release Quickstart (Validated Public Onboarding)

Use this if you want the fastest path from a public release bundle to a real
OpenAI-backed run.

Validated path:

- fresh `ubuntu:24.04`
- install `curl` and `ca-certificates`
- download the public GitHub Release bundle
- put `OPENAI_API_KEY` in `./.env`
- run `./kelvin`

Prerequisites:

- `curl`
- `tar`
- `awk`
- `ca-certificates` on minimal Linux images

Release artifact types:

- Linux: `.tar.gz` bundles and `.deb` packages
- macOS: `.tar.gz` bundles
- Windows: `.zip` bundles

The fully validated onboarding flow today is Linux-based.

Current public release page:

- [GitHub Releases](https://github.com/AgenticHighway/kelvinclaw/releases/latest)

Example for Linux arm64:

```bash
apt-get update && apt-get install -y curl ca-certificates
curl -fsSL -O https://github.com/AgenticHighway/kelvinclaw/releases/download/v0.1.7/kelvinclaw-0.1.7-linux-arm64.tar.gz
curl -fsSL -O https://github.com/AgenticHighway/kelvinclaw/releases/download/v0.1.7/kelvinclaw-0.1.7-linux-arm64.tar.gz.sha256
sha256sum -c kelvinclaw-0.1.7-linux-arm64.tar.gz.sha256
tar -xzf kelvinclaw-0.1.7-linux-arm64.tar.gz
cd kelvinclaw-0.1.7-linux-arm64
printf 'OPENAI_API_KEY=%s\n' '<your_key>' > .env
./kelvin
```

Example for Linux x86_64:

```bash
apt-get update && apt-get install -y curl ca-certificates
curl -fsSL -O https://github.com/AgenticHighway/kelvinclaw/releases/download/v0.1.7/kelvinclaw-0.1.7-linux-x86_64.tar.gz
curl -fsSL -O https://github.com/AgenticHighway/kelvinclaw/releases/download/v0.1.7/kelvinclaw-0.1.7-linux-x86_64.tar.gz.sha256
sha256sum -c kelvinclaw-0.1.7-linux-x86_64.tar.gz.sha256
tar -xzf kelvinclaw-0.1.7-linux-x86_64.tar.gz
cd kelvinclaw-0.1.7-linux-x86_64
printf 'OPENAI_API_KEY=%s\n' '<your_key>' > .env
./kelvin
```

Expected result:

- `./kelvin` fetches the official trust policy
- `kelvin.cli@0.1.1` installs automatically
- `kelvin.openai@0.1.1` installs automatically when `OPENAI_API_KEY` is available
- the no-args run completes with `provider=openai` and `model=gpt-4.1-mini`

Other supported key inputs:

- export `OPENAI_API_KEY` in the shell before running `./kelvin`
- put `OPENAI_API_KEY=...` in `./.env.local`
- put `OPENAI_API_KEY=...` in `~/.kelvinclaw/.env` or `~/.kelvinclaw/.env.local`
- in an interactive terminal, `./kelvin` prompts once if no key is configured

## Canonical Quick Start (Daily Driver MVP)

Local profile (gateway + memory controller + SDK runtime):

```bash
scripts/quickstart.sh --mode local
```

Docker profile:

```bash
scripts/quickstart.sh --mode docker
```

Local profile lifecycle:

```bash
scripts/kelvin-local-profile.sh start
scripts/kelvin-local-profile.sh status
scripts/kelvin-local-profile.sh doctor
scripts/kelvin-local-profile.sh stop
```

Run modes:

- single prompt: `kelvin-host --prompt "hello"`
- interactive chat: `kelvin-host --interactive`
- daemon mode: `scripts/kelvin-local-profile.sh start` (gateway + memory controller background services)

## Track 1: Docker-Only (No Rust/WASM Experience Required)

Use this if you want to run KelvinClaw without installing Rust locally.

Prerequisites:

- `git`
- `docker`

Steps:

```bash
git clone <repo-url>
cd kelvinclaw
scripts/run-runtime-container.sh
```

Optional browser automation profile during container setup:

```bash
KELVIN_SETUP_INSTALL_BROWSER_AUTOMATION=1 scripts/run-runtime-container.sh
```

Verification:

```bash
scripts/verify-onboarding.sh --track beginner
```

Expected result:

- Interactive setup wizard runs on container start.
- Required `kelvin.cli` plugin is installed from plugin index.
- Running `kelvin-host --prompt "hello" --timeout-ms 3000` works without local Rust setup.

Default plugin index URL:

- `https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json`

## Track 2: Rust Developer (Runtime Contributor)

Use this if you are comfortable with Rust and want local compile/test speed.

Prerequisites:

- `git`
- `rustup` + `cargo`
- `jq`
- `curl`
- `tar`
- `openssl`

Steps:

```bash
git clone <repo-url>
cd kelvinclaw
scripts/quickstart.sh --mode local
scripts/test-sdk.sh
```

Verification:

```bash
scripts/verify-onboarding.sh --track rust
```

Expected result:

- SDK test suite passes.
- Local profile boots gateway + memory controller and completes a host run.

## Track 3: Rust + WASM Plugin Author

Use this if you are building or testing WASM plugin modules.

Prerequisites:

- `git`
- `rustup` + `cargo`
- `wasm32-unknown-unknown` target

Docker alternative:

- `docker`
- `scripts/plugin-author-docker.sh` uses a repo-owned Ubuntu 24.04 plugin
  author image so you do not need to start from a blank Ubuntu container

Setup:

```bash
rustup target add wasm32-unknown-unknown
```

Steps:

```bash
git clone <repo-url>
cd kelvinclaw
CARGO_TARGET_DIR=target/echo-wasm-skill cargo build --target wasm32-unknown-unknown --manifest-path examples/echo-wasm-skill/Cargo.toml
cargo run -p kelvin-wasm --bin kelvin-wasm-runner -- --wasm target/echo-wasm-skill/wasm32-unknown-unknown/debug/echo_wasm_skill.wasm --policy-preset locked_down
export PATH="$PWD/scripts:$PATH"
kelvin plugin new --id acme.echo --name "Acme Echo" --runtime wasm_tool_v1
kelvin plugin test --manifest ./plugin-acme.echo/plugin.json
```

For the supported model-plugin contributor path, use:

- `docs/build-a-model-plugin.md`
- `examples/kelvin-anthropic-plugin`
- `examples/kelvin-openrouter-plugin`

Docker-first authoring shortcut:

```bash
git clone <repo-url>
cd kelvinclaw
scripts/plugin-author-docker.sh -- scripts/test-plugin-author-kit.sh
```

Verification:

```bash
scripts/verify-onboarding.sh --track wasm
```

Expected result:

- Sample WASM skill builds successfully.
- WASM runner executes the module under sandbox policy.
- Plugin author commands scaffold and validate plugin package structure without touching root crates.
- `kelvin plugin install` and `kelvin plugin smoke` cover the local package-install and model-runtime smoke path without requiring host flag memorization.
- Model plugins can be scaffolded, built, packed, and locally installed through the same public SDK surface.

## Verify All Tracks

Run full onboarding verification:

```bash
scripts/verify-onboarding.sh --track all
scripts/verify-onboarding.sh --track daily
```

`all` runs `beginner`, `rust`, and `wasm`. `daily` validates the default daily-driver local profile with a time-to-first-success threshold.

## Security and Stability Notes

- Plugin execution is policy-gated and signature-verified by default.
- First-party CLI plugin installation uses the same installed-plugin flow as other plugins.
- Onboarding verification intentionally checks runtime behavior and SDK tests, not only tool presence.
