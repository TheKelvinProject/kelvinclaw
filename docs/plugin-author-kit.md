# Plugin Author Kit

Kelvin provides an authoring flow that does not require modifying root crates or
reading Kelvin runtime internals.

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

If you want a reproducible container instead of local Rust setup, use the
repo-owned Ubuntu 24.04 author image through:

```bash
scripts/plugin-author-docker.sh -- bash
```

That is the supported Docker authoring path for plugin contributors.

## Minimal Flow

```bash
scripts/kelvin-plugin.sh new --id acme.echo --name "Acme Echo" --runtime wasm_tool_v1
scripts/kelvin-plugin.sh test --manifest ./plugin-acme.echo/plugin.json
scripts/kelvin-plugin.sh pack --manifest ./plugin-acme.echo/plugin.json
scripts/kelvin-plugin.sh verify --package ./plugin-acme.echo/dist/acme.echo-0.1.0.tar.gz
```

For model plugins, use the dedicated guide:

- [docs/build-a-model-plugin.md](build-a-model-plugin.md)

`kelvin plugin new --runtime wasm_model_v1` now creates a working source project
with:

- `plugin.json`
- `src/lib.rs`
- `build.sh`
- a compiled `payload/plugin.wasm`

## Templates

Reference templates:

- `templates/plugin-author-kit/wasm_tool/plugin.json.template`
- `templates/plugin-author-kit/wasm_model/plugin.json.template`

For new model plugins, prefer the generic host-routed `provider_profile` field (`openai.responses`, `anthropic.messages`) instead of the legacy provider-specific host import.

The maintained example source crate is:

- `examples/kelvin-anthropic-plugin`

Use the example in Docker with:

```bash
scripts/plugin-author-docker.sh -- bash -lc 'cd examples/kelvin-anthropic-plugin && ./build.sh'
```

## Signing

Local/community development plugins can stay `unsigned_local`. Kelvin warns on
install for that quality tier, but still allows the plugin to install.

```bash
AWS_PROFILE=REDACTED_AWS_PROFILE scripts/plugin-sign.sh \
  --manifest ./plugin-acme.echo/plugin.json \
  --kms-key-id REDACTED_KMS_ALIAS \
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
