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

Optional field:

- `entrypoint_sha256` (recommended integrity check)

## Install Command

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

## Verification

Run plugin lifecycle tests:

```bash
scripts/test-plugin-install.sh
```
