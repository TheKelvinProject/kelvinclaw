# Plugin Install Flow (No Local Compilation)

This flow is for end users installing prebuilt SDK plugins.

Goal:

- users install a plugin package
- users do not compile Rust locally
- plugin artifacts are isolated from Kelvin root code

## Package Format

A plugin package is a `.tar.gz` with:

- `plugin.json`
- `payload/<files...>`

`plugin.json` required fields:

- `id`
- `name`
- `version`
- `api_version`
- `entrypoint` (relative path under `payload/`)
- `capabilities` (non-empty list)

Optional fields:

- `entrypoint_sha256` (recommended integrity check)
- `publisher` (required when signature verification is enforced)
- `runtime` (`wasm_tool_v1` or `wasm_model_v1`)
- `tool_name` (tool runtime)
- `provider_name` + `model_name` (model runtime)
- `provider_profile` (recommended for generic model runtime host routing)
- `capability_scopes` / `operational_controls`
- `quality_tier` (`unsigned_local`, `signed_community`, `signed_trusted`)

Optional package file:

- `plugin.sig` (Ed25519 signature over `plugin.json`)

## Install Command

Install Kelvin's first-party CLI plugin package:

```bash
scripts/install-kelvin-cli-plugin.sh
```

Install Kelvin's first-party OpenAI model plugin package:

```bash
scripts/install-kelvin-openai-plugin.sh
```

Install optional browser automation plugin profile:

```bash
scripts/install-kelvin-browser-plugin.sh
```

Generic package install:

```bash
scripts/plugin-install.sh --package ./dist/acme.echo-1.0.0.tar.gz
```

`unsigned_local` and `signed_community` packages are still installable. Kelvin
prints a warning so community authors can develop locally without access to the
first-party signing platform.

Install from remote plugin index:

```bash
scripts/plugin-index-install.sh --plugin kelvin.cli
scripts/plugin-update-check.sh --json
```

Discover index entries:

```bash
scripts/plugin-discovery.sh
scripts/plugin-discovery.sh --plugin kelvin.cli
```

Run the hosted registry service instead of a raw `index.json`:

```bash
cargo run -p kelvin-registry -- --index ./index.json --bind 127.0.0.1:34718
scripts/plugin-discovery.sh --registry-url http://127.0.0.1:34718
scripts/plugin-index-install.sh --plugin kelvin.cli --registry-url http://127.0.0.1:34718
scripts/plugin-update-check.sh --registry-url http://127.0.0.1:34718 --json
```

Default index URL:

- `https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json`

Default install location:

- `~/.kelvinclaw/plugins/<plugin_id>/<version>/`
- symlink: `~/.kelvinclaw/plugins/<plugin_id>/current -> <version>`

Override install root:

```bash
KELVIN_PLUGIN_HOME=./.kelvin/plugins scripts/plugin-install.sh --package ./dist/acme.echo-1.0.0.tar.gz
```

Optional runtime env overrides:

- `KELVIN_PLUGIN_HOME`
- `KELVIN_TRUST_POLICY_PATH`

## List Installed Plugins

Table output:

```bash
scripts/plugin-list.sh
```

JSON output:

```bash
scripts/plugin-list.sh --json
```

## Uninstall Plugin

Remove one version:

```bash
scripts/plugin-uninstall.sh --id acme.echo --version 1.0.0
```

Remove all versions for a plugin id:

```bash
scripts/plugin-uninstall.sh --id acme.echo --purge
```

## Validation Performed by Installer

- package structure exists (`plugin.json`, `payload/`)
- required manifest fields parse
- safe relative entrypoint path
- entrypoint file exists
- optional SHA-256 match (if provided)
- duplicate install protection (unless `--force`)

## Why This Is Privacy-Conscious

- no personal paths or host data in plugin artifacts
- no compilation step on user machine
- install root is local and user-scoped by default

## Runtime Security Notes

Install-time checks validate package integrity and structure. Runtime checks in `kelvin-brain` additionally enforce:

- trusted publisher signature verification (when enabled)
- capability scope allowlists
- execution timeout/retry/rate/circuit controls

## Publisher Signing Workflow

Generate `plugin.sig` from `plugin.json` and emit trust policy snippet:

```bash
AWS_PROFILE=REDACTED_AWS_PROFILE scripts/plugin-sign.sh \
  --manifest ./plugin.json \
  --kms-key-id REDACTED_KMS_ALIAS \
  --kms-region us-east-1 \
  --publisher-id acme \
  --trust-policy-out ./trusted_publishers.acme.json
```

PEM signing remains available for community publishers:

```bash
scripts/plugin-sign.sh \
  --manifest ./plugin.json \
  --private-key ./acme-ed25519-private.pem \
  --publisher-id acme \
  --trust-policy-out ./trusted_publishers.acme.json
```

Reference template:

- `trusted_publishers.example.json`
- `https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/trusted_publishers.kelvin.json`

## Verification

Run plugin lifecycle tests:

```bash
scripts/test-plugin-install.sh
```

Authoring/packaging flow:

```bash
scripts/kelvin-plugin.sh new --id acme.echo --name "Acme Echo" --runtime wasm_tool_v1
scripts/kelvin-plugin.sh test --manifest ./plugin-acme.echo/plugin.json
scripts/kelvin-plugin.sh pack --manifest ./plugin-acme.echo/plugin.json
scripts/kelvin-plugin.sh verify --package ./plugin-acme.echo/dist/acme.echo-0.1.0.tar.gz
```

Trust policy operations:

```bash
scripts/plugin-trust.sh show
scripts/plugin-trust.sh rotate-key --publisher acme --public-key <base64>
scripts/plugin-trust.sh revoke --publisher acme
scripts/plugin-trust.sh pin --plugin acme.echo --publisher acme
```
