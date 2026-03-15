# Kelvin OpenRouter Model Plugin

This example shows the declarative provider-profile path for providers that fit
an existing protocol family. It uses the public `wasm_model_v1` SDK surface and
declares an OpenRouter profile on the `openai_chat_completions` protocol
family.

Quick commands:

```bash
./build.sh
../../scripts/kelvin-plugin.sh test --manifest ./plugin.json
../../scripts/kelvin-plugin.sh pack --manifest ./plugin.json
../../scripts/kelvin-plugin.sh verify --package ./dist/kelvin.openrouter-0.1.0.tar.gz
```

For local development this plugin intentionally stays `unsigned_local`. Kelvin
will warn on install, but still allow it to load from a local plugin home.
