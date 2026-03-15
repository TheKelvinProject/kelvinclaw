# OpenRouter Plugin Install and Run

This guide runs Kelvin with the first-party OpenRouter model plugin on the SDK
path.

## Prerequisites

- `OPENROUTER_API_KEY` set in your shell.
- Installed plugin trust policy and plugin home (defaults are fine).
- CLI plugin installed (required preflight in `kelvin-sdk` runtime composition).

## Install Plugins

Install the CLI plugin:

```bash
scripts/install-kelvin-cli-plugin.sh
```

Install the OpenRouter model plugin:

```bash
scripts/install-kelvin-openrouter-plugin.sh
```

Default index URL:

- `https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json`

Both installers support overrides:

- `KELVIN_PLUGIN_HOME`
- `KELVIN_TRUST_POLICY_PATH`

## Run Kelvin with OpenRouter Provider

```bash
export OPENROUTER_API_KEY="<your_key>"

cargo run -p kelvin-host -- \
  --prompt "Summarize KelvinClaw in one sentence." \
  --model-provider kelvin.openrouter \
  --memory fallback
```

Expected behavior:

- runtime loads installed plugins through signature + manifest checks
- model provider is selected explicitly by plugin id (`kelvin.openrouter`)
- request executes through the generic `provider_profile_call` guest ABI
- host resolves the declarative `openrouter.chat` provider profile object
- host normalizes the request through `openai_chat_completions`

## Deterministic Mock Test (No Live API Key)

Run mock-backed SDK integration test:

```bash
cargo test -p kelvin-sdk --lib run_with_sdk_uses_installed_openrouter_model_plugin_via_mock_server -- --nocapture
```

## Failure Modes

- missing plugin id or install path: typed configuration/load error
- missing `OPENROUTER_API_KEY`: typed invalid-input error before outbound call
- host not in allowlist: typed invalid-input error
- provider 4xx/5xx: typed backend error
- malformed plugin output: typed invalid-input error
