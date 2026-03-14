# Plugin Author Kit

Kelvin provides an authoring flow that does not require modifying root crates.

## Commands

Add `scripts/` to `PATH`:

```bash
export PATH="$PWD/scripts:$PATH"
```

Then use:

```bash
kelvin plugin new
kelvin plugin test
kelvin plugin pack
kelvin plugin verify
```

Equivalent direct command:

```bash
scripts/kelvin-plugin.sh <new|test|pack|verify> ...
```

## Minimal Flow

```bash
scripts/kelvin-plugin.sh new --id acme.echo --name "Acme Echo" --runtime wasm_tool_v1
scripts/kelvin-plugin.sh test --manifest ./plugin-acme.echo/plugin.json
scripts/kelvin-plugin.sh pack --manifest ./plugin-acme.echo/plugin.json
scripts/kelvin-plugin.sh verify --package ./plugin-acme.echo/dist/acme.echo-0.1.0.tar.gz
```

## Templates

Reference templates:

- `templates/plugin-author-kit/wasm_tool/plugin.json.template`
- `templates/plugin-author-kit/wasm_model/plugin.json.template`

For new model plugins, prefer the generic host-routed `provider_profile` field (`openai.responses`, `anthropic.messages`) instead of the legacy provider-specific host import.

## Signing

```bash
AWS_PROFILE=ah-willsarg-iam scripts/plugin-sign.sh \
  --manifest ./plugin-acme.echo/plugin.json \
  --kms-key-id alias/ah/kelvin/plugins/prod \
  --kms-region us-east-1 \
  --publisher-id acme \
  --trust-policy-out ./trusted_publishers.acme.json
```

For non-AgenticHighway publishers, the local PEM flow remains available:

```bash
scripts/plugin-sign.sh \
  --manifest ./plugin-acme.echo/plugin.json \
  --private-key /path/to/ed25519-private.pem \
  --publisher-id acme \
  --trust-policy-out ./trusted_publishers.acme.json
```

## Compatibility Matrix

`kelvin plugin test` checks plugin compatibility against one or more core versions:

```bash
scripts/kelvin-plugin.sh test --manifest ./plugin.json --core-versions "0.1.0,0.2.0"
```

This is deterministic and intended for CI gates.
