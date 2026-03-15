# Kelvin Plugin Author Kit (Template)

This template directory is a reference starting point for third-party plugin authors.

Primary command flow:

```bash
scripts/kelvin-plugin.sh new --id acme.echo --name "Acme Echo" --runtime wasm_tool_v1
scripts/kelvin-plugin.sh test --manifest ./plugin-acme.echo/plugin.json
scripts/kelvin-plugin.sh pack --manifest ./plugin-acme.echo/plugin.json
scripts/kelvin-plugin.sh verify --package ./plugin-acme.echo/dist/acme.echo-0.1.0.tar.gz
```

For working model-plugin source, also see:

- `examples/kelvin-anthropic-plugin`
- `examples/kelvin-openrouter-plugin`
- `docs/build-a-model-plugin.md`

Template manifests:

- `wasm_tool/plugin.json.template`
- `wasm_model/plugin.json.template`

New model plugins should declare a structured `provider_profile` object. Kelvin
core routes and adapts requests by `protocol_family`, so most new providers only
need manifest changes, not host-runtime changes.

The author-kit templates default to `unsigned_local` so community contributors can
build and install plugins locally without access to AgenticHighway's signing
platform. Kelvin warns on install for unsigned local packages, but still allows
them to load from a local plugin home.

Signing and trust policy:

```bash
AWS_PROFILE=REDACTED_AWS_PROFILE scripts/plugin-sign.sh --manifest ./plugin.json --kms-key-id REDACTED_KMS_ALIAS --kms-region us-east-1 --publisher-id acme --trust-policy-out ./trusted_publishers.acme.json
```

Community publishers can continue using `--private-key /path/to/private.pem` instead of `--kms-key-id`.
