# Anthropic Plugin Install and Run

This guide runs Kelvin with the first-party Anthropic model plugin on the SDK path.

## Prerequisites

- `ANTHROPIC_API_KEY` set in your shell.
- Installed plugin trust policy and plugin home (defaults are fine).
- CLI plugin installed (required preflight in `kelvin-sdk` runtime composition).

## Install Plugins

Install the CLI plugin:

```bash
scripts/install-kelvin-cli-plugin.sh
```

Install the Anthropic model plugin:

```bash
scripts/install-kelvin-anthropic-plugin.sh
```

Default index URL:

- `https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json`

Both installers support overrides:

- `KELVIN_PLUGIN_HOME`
- `KELVIN_TRUST_POLICY_PATH`

## Run Kelvin with Anthropic Provider

```bash
export ANTHROPIC_API_KEY="<your_key>"

cargo run -p kelvin-host -- \
  --prompt "Summarize KelvinClaw in one sentence." \
  --model-provider kelvin.anthropic \
  --memory fallback
```

Expected behavior:

- runtime loads installed plugins through signature + manifest checks
- model provider is selected explicitly by plugin id (`kelvin.anthropic`)
- request executes through the generic `provider_profile_call` guest ABI
- host resolves the `anthropic.messages` provider profile and performs the HTTPS call

## Failure Modes

- missing plugin id or install path: typed configuration/load error
- missing `ANTHROPIC_API_KEY`: typed invalid-input error before outbound call
- host not in allowlist: typed invalid-input error
- provider 4xx/5xx: typed backend error
- malformed plugin output: typed invalid-input error
