# Plugin Index Schema (v1)

Kelvin runtime can install plugins from a remote index using:

```bash
scripts/plugin-index-install.sh --plugin <id>
```

Default index URL:

- `https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json`

## Schema

```json
{
  "schema_version": "v1",
  "plugins": [
    {
      "id": "kelvin.cli",
      "version": "0.1.0",
      "package_url": "https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/packages/kelvin.cli/0.1.0/kelvin.cli-0.1.0.tar.gz",
      "sha256": "7db6...<64 hex chars>...",
      "trust_policy_url": "https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/trusted_publishers.kelvin.json",
      "quality_tier": "signed_trusted",
      "tags": ["first_party", "cli"]
    }
  ]
}
```

Field requirements:

- `schema_version`: required, must be `v1`
- `plugins`: required array
- per plugin entry:
  - `id`: required
  - `version`: required
  - `package_url`: required
  - `sha256`: required (fail-closed if missing/mismatch)
  - `trust_policy_url`: optional
  - `quality_tier`: optional (`unsigned_local`, `signed_community`, `signed_trusted`)
  - `tags`: optional string array for discovery/category

Selection behavior:

- `--plugin <id>` required
- `--version <version>` optional
- if version is omitted, installer chooses the highest dotted semver release
- optional minimum quality gate via `--min-quality-tier` or `KELVIN_PLUGIN_MIN_QUALITY_TIER`

## Hosted Registry API

`apps/kelvin-registry` serves the same `v1` index plus filtered discovery endpoints:

- `GET /healthz`
- `GET /v1/index.json`
- `GET /v1/plugins`
- `GET /v1/plugins/{plugin_id}`
- `GET /v1/trust-policy`

Example:

```bash
cargo run -p kelvin-registry -- --index ./index.json --bind 127.0.0.1:34718
scripts/plugin-discovery.sh --registry-url http://127.0.0.1:34718
scripts/plugin-index-install.sh --plugin kelvin.cli --registry-url http://127.0.0.1:34718
scripts/plugin-update-check.sh --registry-url http://127.0.0.1:34718 --json
```

## Trust Policy

If `trust_policy_url` is present, installer fetches and merges it into local trust policy:

- `require_signature` remains strict (`base && incoming`)
- `publishers` merged by `id` (last entry wins for duplicates)

This keeps runtime signature verification strict by default.

## Discovery

Registry discovery helper:

```bash
scripts/plugin-discovery.sh
scripts/plugin-discovery.sh --plugin kelvin.cli
scripts/plugin-discovery.sh --json
scripts/plugin-update-check.sh --json
```
