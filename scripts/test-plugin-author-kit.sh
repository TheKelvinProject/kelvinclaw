#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN_CLI="${ROOT_DIR}/scripts/kelvin-plugin.sh"

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 1
  fi
}

sha256_file() {
  local file="$1"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${file}" | awk '{print $1}'
    return
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${file}" | awk '{print $1}'
    return
  fi
  echo "Missing required command: shasum or sha256sum" >&2
  exit 1
}

require_cmd jq
require_cmd tar
require_cmd cargo
require_cmd rustup

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "${WORK_DIR}"' EXIT

pushd "${WORK_DIR}" >/dev/null

"${PLUGIN_CLI}" new --id acme.echo --name "Acme Echo" --runtime wasm_tool_v1 --out ./plugin-acme
printf 'fake-wasm' > ./plugin-acme/payload/plugin.wasm
SHA="$(sha256_file ./plugin-acme/payload/plugin.wasm)"
jq --arg sha "${SHA}" '.entrypoint_sha256 = $sha' ./plugin-acme/plugin.json > ./plugin-acme/plugin.json.tmp
mv ./plugin-acme/plugin.json.tmp ./plugin-acme/plugin.json

"${PLUGIN_CLI}" test --manifest ./plugin-acme/plugin.json --core-versions "0.1.0,0.2.0"
"${PLUGIN_CLI}" pack --manifest ./plugin-acme/plugin.json
"${PLUGIN_CLI}" verify --package ./plugin-acme/dist/acme.echo-0.1.0.tar.gz

"${PLUGIN_CLI}" new --id acme.anthropic --name "Acme Anthropic" --runtime wasm_model_v1 --provider-profile anthropic.messages --out ./plugin-anthropic
"${PLUGIN_CLI}" test --manifest ./plugin-anthropic/plugin.json --core-versions "0.1.0,0.2.0"
"${PLUGIN_CLI}" pack --manifest ./plugin-anthropic/plugin.json
"${PLUGIN_CLI}" verify --package ./plugin-anthropic/dist/acme.anthropic-0.1.0.tar.gz

"${PLUGIN_CLI}" new \
  --id acme.openrouter \
  --name "Acme OpenRouter" \
  --runtime wasm_model_v1 \
  --provider-name openrouter \
  --provider-profile openrouter.chat \
  --protocol-family openai_chat_completions \
  --api-key-env OPENROUTER_API_KEY \
  --base-url-env OPENROUTER_BASE_URL \
  --default-base-url https://openrouter.ai/api/v1 \
  --endpoint-path chat/completions \
  --allow-host openrouter.ai \
  --model-name openai/gpt-4.1-mini \
  --out ./plugin-openrouter
"${PLUGIN_CLI}" test --manifest ./plugin-openrouter/plugin.json --core-versions "0.1.0,0.2.0"
"${PLUGIN_CLI}" pack --manifest ./plugin-openrouter/plugin.json
"${PLUGIN_CLI}" verify --package ./plugin-openrouter/dist/acme.openrouter-0.1.0.tar.gz

# Signed-community tier should fail without plugin.sig.
jq '.quality_tier = "signed_community" | .publisher = "acme"' ./plugin-acme/plugin.json > ./plugin-acme/plugin.json.tmp
mv ./plugin-acme/plugin.json.tmp ./plugin-acme/plugin.json
if "${PLUGIN_CLI}" verify --manifest ./plugin-acme/plugin.json >/dev/null 2>&1; then
  echo "Expected signed_community verify to fail without plugin.sig" >&2
  exit 1
fi

popd >/dev/null
echo "[test-plugin-author-kit] success"
