# Kelvin Anthropic Model Plugin

This example is the canonical first-party `wasm_model_v1` source crate for the
public Kelvin plugin-authoring flow. Community contributors can copy this
directory, rename the manifest fields, and replace the structured
`provider_profile` object with their own host-routed model profile.

Quick commands:

```bash
./build.sh
../../scripts/kelvin-plugin.sh test --manifest ./plugin.json
../../scripts/kelvin-plugin.sh pack --manifest ./plugin.json
../../scripts/kelvin-plugin.sh verify --package ./dist/kelvin.anthropic-0.1.0.tar.gz
```

For local development this plugin intentionally stays `unsigned_local`. Kelvin
will warn on install, but still allow it to load from a local plugin home.
