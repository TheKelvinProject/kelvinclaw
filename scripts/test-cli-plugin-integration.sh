#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"
WORK_DIR="$(mktemp -d)"
PLUGIN_HOME="${WORK_DIR}/plugins"
TRUST_POLICY_PATH="${WORK_DIR}/trusted_publishers.json"
TARGET_DIR="${KELVIN_TEST_TARGET_DIR:-${WORK_DIR}/target}"
LOG_PATH="${WORK_DIR}/cli.log"
PLUGIN_INDEX_URL="${KELVIN_PLUGIN_INDEX_URL:-https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json}"

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "[test-cli-plugin-integration] missing required command: ${name}" >&2
    exit 1
  fi
}

cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

require_cmd jq
if ! ensure_rust_toolchain_path; then
  echo "[test-cli-plugin-integration] missing required commands: cargo/rustup" >&2
  exit 1
fi
require_cmd cargo

echo "[test-cli-plugin-integration] installing kelvin_cli package from plugin index"
"${ROOT_DIR}/scripts/install-kelvin-cli-plugin.sh" \
  --index-url "${PLUGIN_INDEX_URL}" \
  --plugin-home "${PLUGIN_HOME}" \
  --trust-policy-path "${TRUST_POLICY_PATH}"

echo "[test-cli-plugin-integration] running kelvin-host via SDK"
KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" \
KELVIN_TRUST_POLICY_PATH="${TRUST_POLICY_PATH}" \
CARGO_TARGET_DIR="${TARGET_DIR}" \
  cargo run -p kelvin-host -- \
    --prompt "integration sdk lane" \
    --timeout-ms 5000 > "${LOG_PATH}"

if ! grep -q "cli plugin preflight: kelvin_cli executed" "${LOG_PATH}"; then
  echo "[test-cli-plugin-integration] expected cli plugin preflight output not found" >&2
  cat "${LOG_PATH}" >&2
  exit 1
fi

echo "[test-cli-plugin-integration] success"
