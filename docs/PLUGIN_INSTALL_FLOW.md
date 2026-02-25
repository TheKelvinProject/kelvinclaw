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
- `capability_scopes` / `operational_controls`

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

Generic package install:

```bash
scripts/plugin-install.sh --package ./dist/acme.echo-1.0.0.tar.gz
```

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
scripts/plugin-sign.sh \
  --manifest ./plugin.json \
  --private-key ./acme-ed25519-private.pem \
  --publisher-id acme \
  --trust-policy-out ./trusted_publishers.acme.json
```

Reference template:

- `trusted_publishers.example.json`
- `plugins/trusted_publishers.kelvin.json` (bundled Kelvin publisher key)

## Verification

Run plugin lifecycle tests:

```bash
scripts/test-plugin-install.sh
```
