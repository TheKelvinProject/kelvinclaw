# OpenAI Plugin Install and Run

This guide runs Kelvin with the first-party OpenAI model plugin on the SDK path.

## Prerequisites

- `OPENAI_API_KEY` set in your shell.
- Installed plugin trust policy and plugin home (defaults are fine).
- CLI plugin installed (required preflight in `kelvin-sdk` runtime composition).

## Install Plugins

Install the CLI plugin:

```bash
scripts/install-kelvin-cli-plugin.sh
```

Install the OpenAI model plugin:

```bash
scripts/install-kelvin-openai-plugin.sh
```

Default index URL:

- `https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json`

Both installers support overrides:

- `KELVIN_PLUGIN_HOME`
- `KELVIN_TRUST_POLICY_PATH`

## Run Kelvin with OpenAI Provider

```bash
export OPENAI_API_KEY="<your_key>"

cargo run -p kelvin-host -- \
  --prompt "Summarize KelvinClaw in one sentence." \
  --model-provider kelvin.openai \
  --memory fallback
```

Expected behavior:

- runtime loads installed plugins through signature + manifest checks
- model provider is selected explicitly by plugin id (`kelvin.openai`)
- request executes through `wasm_model_v1` guest ABI
- host performs the OpenAI HTTPS call and returns typed output

## Deterministic Mock Test (No Live API Key)

Run mock-backed SDK integration test:

```bash
cargo test -p kelvin-sdk --lib run_with_sdk_uses_installed_openai_model_plugin_via_mock_server -- --nocapture
```

This validates the full SDK + WASM model-provider path without live network secrets.

## Failure Modes

- missing plugin id or install path: typed configuration/load error
- missing `OPENAI_API_KEY`: typed invalid-input error before outbound call
- host not in allowlist: typed invalid-input error
- provider 4xx/5xx: typed backend error
- malformed plugin output: typed invalid-input error

No silent fallback is performed when `--model-provider` is set.
