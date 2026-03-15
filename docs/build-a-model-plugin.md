# Build a Model Plugin

This is the supported KelvinClaw contributor path for new `wasm_model_v1`
plugins. You do not need to modify Kelvin core internals to follow it.

## Prerequisites

- `cargo`
- `rustup`
- `jq`

If you do not want to install Rust locally, the supported Docker path uses the
repo-owned Ubuntu 24.04 plugin-author image:

```bash
scripts/plugin-author-docker.sh -- bash
```

That wrapper builds a cached local image, mounts the repository, and reuses
repo-local Cargo registry/git/target caches for fast iteration.

## Option 1: Scaffold a New Model Plugin

```bash
scripts/kelvin-plugin.sh new \
  --id acme.anthropic \
  --name "Acme Anthropic Model Plugin" \
  --runtime wasm_model_v1 \
  --provider-profile anthropic.messages
```

That command now creates:

- a valid `plugin.json`
- a tiny Rust guest in `src/lib.rs`
- a local `build.sh`
- a compiled `payload/plugin.wasm`

Then iterate with:

```bash
cd ./plugin-acme.anthropic
./build.sh
../scripts/kelvin-plugin.sh test --manifest ./plugin.json
../scripts/kelvin-plugin.sh pack --manifest ./plugin.json
../scripts/kelvin-plugin.sh verify --package ./dist/acme.anthropic-0.1.0.tar.gz
```

The same flow in Docker:

```bash
scripts/plugin-author-docker.sh -- bash -lc '
  scripts/kelvin-plugin.sh new \
    --id acme.anthropic \
    --name "Acme Anthropic Model Plugin" \
    --runtime wasm_model_v1 \
    --provider-profile anthropic.messages
  cd ./plugin-acme.anthropic
  ./build.sh
  ../scripts/kelvin-plugin.sh test --manifest ./plugin.json
  ../scripts/kelvin-plugin.sh pack --manifest ./plugin.json
  ../scripts/kelvin-plugin.sh verify --package ./dist/acme.anthropic-0.1.0.tar.gz
'
```

## Option 2: Copy the Maintained Example

The canonical first-party model-plugin example is:

- `examples/kelvin-anthropic-plugin`
- `examples/kelvin-openrouter-plugin`

You can copy it, rename the manifest fields, and adjust:

- `id`
- `name`
- `provider_name`
- `provider_profile`
- `model_name`
- `network_allow_hosts`

For a non-builtin provider that still fits an existing protocol family, scaffold
the structured profile directly:

```bash
scripts/kelvin-plugin.sh new \
  --id acme.openrouter \
  --name "Acme OpenRouter" \
  --runtime wasm_model_v1 \
  --provider-name openrouter \
  --provider-profile openrouter.chat \
  --protocol-family openai_chat_completions \
  --api-key-env OPENROUTER_API_KEY \
  --base-url-env OPENROUTER_BASE_URL \
  --default-base-url https://openrouter.ai/api/v1 \
  --endpoint-path chat/completions \
  --allow-host openrouter.ai \
  --model-name openai/gpt-4.1-mini
```

## Local Install And Run

Local development plugins can stay `unsigned_local`:

```bash
scripts/plugin-install.sh --package ./dist/acme.anthropic-0.1.0.tar.gz
```

Kelvin prints a warning for `unsigned_local`, but still installs the package so
you can develop without access to the first-party signing platform.

Run Kelvin with the plugin:

```bash
KELVIN_PLUGIN_HOME="${HOME}/.kelvinclaw/plugins" \
KELVIN_TRUST_POLICY_PATH="${HOME}/.kelvinclaw/trusted_publishers.json" \
cargo run -p kelvin-host -- \
  --prompt "Summarize KelvinClaw in one sentence." \
  --model-provider acme.anthropic \
  --memory fallback
```

If you are validating in Docker, run the same commands through
`scripts/plugin-author-docker.sh -- ...` so the repo path and caches stay
consistent.

## Publishing

Local/community development happens in source repos like `kelvinclaw` or your
own plugin repo. The `kelvinclaw-plugins` repository is only for published
artifacts:

- package tarballs
- `index.json`
- trust metadata

Only AgenticHighway first-party releases currently use the official KMS signing
platform. Community authors can keep using unsigned local plugins or their own
PEM signing flow.
