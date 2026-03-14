# Kelvin Plugin Author Kit (Template)

This template directory is a reference starting point for third-party plugin authors.

Primary command flow:

```bash
scripts/kelvin-plugin.sh new --id acme.echo --name "Acme Echo" --runtime wasm_tool_v1
scripts/kelvin-plugin.sh test --manifest ./plugin-acme.echo/plugin.json
scripts/kelvin-plugin.sh pack --manifest ./plugin-acme.echo/plugin.json
scripts/kelvin-plugin.sh verify --package ./plugin-acme.echo/dist/acme.echo-0.1.0.tar.gz
```

Template manifests:

- `wasm_tool/plugin.json.template`
- `wasm_model/plugin.json.template`

New model plugins should declare a `provider_profile` such as `openai.responses` or `anthropic.messages` so the host can enforce provider routing and auth policy.

Signing and trust policy:

```bash
scripts/plugin-sign.sh --manifest ./plugin.json --private-key /path/to/private.pem --publisher-id acme --trust-policy-out ./trusted_publishers.acme.json
```
