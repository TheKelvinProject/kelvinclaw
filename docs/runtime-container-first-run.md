# Runtime Container First Run

This flow is for end users who should not need Rust or `cargo`.

## Goal

- run Kelvin from a prebuilt/minimal runtime image
- complete first-time setup in an interactive terminal
- install required plugins from a remote plugin repository index

## Build and Run Local Runtime Image

```bash
scripts/run-runtime-container.sh
```

What this does:

- builds `docker/Dockerfile.runtime` (minimal runtime image)
- starts an interactive container
- runs `scripts/kelvin-setup.sh` automatically
- installs required plugin `kelvin.cli` from the configured index URL (default or override)
- optionally installs `kelvin.browser.automation` when `KELVIN_SETUP_INSTALL_BROWSER_AUTOMATION=1`

Container defaults:

- `KELVIN_HOME=/kelvin`
- `KELVIN_PLUGIN_HOME=/kelvin/plugins`
- `KELVIN_TRUST_POLICY_PATH=/kelvin/trusted_publishers.json`

The script mounts:

- repo `.kelvin/` -> `/kelvin` (persists setup/plugins between runs)
- repo root -> `/workspace`

## Running Kelvin in the Container

After setup:

```bash
kelvin-host --prompt "What is KelvinClaw?" --timeout-ms 3000
```

## Non-Interactive Setup

```bash
scripts/kelvin-setup.sh --non-interactive
```

Default plugin index URL:

- `https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json`

## Security Notes

- `scripts/plugin-index-install.sh` requires `sha256` in index entries and fails closed on mismatch.
- Plugin install path uses `scripts/plugin-install.sh` (manifest + payload checks).
- Runtime admission still enforces trusted publisher signatures via trust policy at load time.
