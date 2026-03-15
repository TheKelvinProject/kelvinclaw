#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_JSON="${ROOT_DIR}/plugin.json"
PAYLOAD_DIR="${ROOT_DIR}/payload"
ENTRYPOINT_REL="$(jq -er '.entrypoint' "${PLUGIN_JSON}")"
ENTRYPOINT_ABS="${PAYLOAD_DIR}/${ENTRYPOINT_REL}"
TARGET_ROOT="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
TARGET_DIR="${TARGET_ROOT}/wasm32-unknown-unknown/release"
WASM_SOURCE="${TARGET_DIR}/kelvin_openrouter_plugin.wasm"

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

require_cmd cargo
require_cmd jq
require_cmd rustup

rustup target add wasm32-unknown-unknown >/dev/null
cargo build --release --target wasm32-unknown-unknown

mkdir -p "$(dirname "${ENTRYPOINT_ABS}")"
cp "${WASM_SOURCE}" "${ENTRYPOINT_ABS}"

ENTRYPOINT_SHA="$(sha256_file "${ENTRYPOINT_ABS}")"
jq --arg sha "${ENTRYPOINT_SHA}" '.entrypoint_sha256 = $sha' "${PLUGIN_JSON}" > "${PLUGIN_JSON}.tmp"
mv "${PLUGIN_JSON}.tmp" "${PLUGIN_JSON}"

echo "[kelvin-plugin] built kelvin.openrouter -> ${ENTRYPOINT_ABS}"
echo "[kelvin-plugin] entrypoint sha256: ${ENTRYPOINT_SHA}"
