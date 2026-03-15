# Plugin Quality Tiers

Kelvin plugin manifests may declare `quality_tier`:

- `unsigned_local`
  - local development and experimentation only
  - no signature required
  - Kelvin warns on install, but still allows local installation
- `signed_community`
  - signed package expected (`plugin.sig`)
  - non-empty `publisher` required
- `signed_trusted`
  - signed package expected (`plugin.sig`)
  - non-empty `publisher` required
  - trust policy membership required for verification gates

## Verification

`scripts/kelvin-plugin.sh verify` enforces tier-specific checks.
`scripts/plugin-install.sh` prints an install-time warning for `unsigned_local`
and `signed_community`, but does not block local installation.

For trusted tier:

```bash
scripts/kelvin-plugin.sh verify \
  --package ./dist/acme.echo-1.0.0.tar.gz \
  --trust-policy ./trusted_publishers.json
```

## Runtime Policy Tie-In

Installed plugin trust policy supports:

- revoked publishers
- plugin-to-publisher pinning

These controls are enforced by installed plugin loading (`kelvin-brain`).
