#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"

DEFAULT_INDEX_URL="https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json"
INDEX_URL="${KELVIN_PLUGIN_INDEX_URL:-${DEFAULT_INDEX_URL}}"
REGISTRY_URL="${KELVIN_PLUGIN_REGISTRY_URL:-}"
PLUGIN_ID=""

usage() {
  cat <<'USAGE'
Usage: scripts/test-plugin-abi-compat.sh --plugin <id> [options]

Install a plugin from the external registry/index and verify Kelvin can load it through the SDK path.

Options:
  --plugin <id>         Plugin id to install and verify
  --index-url <url>     Plugin index JSON URL
  --registry-url <url>  Hosted registry base URL
  -h, --help            Show help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --plugin)
      PLUGIN_ID="${2:?missing value for --plugin}"
      shift 2
      ;;
    --index-url)
      INDEX_URL="${2:?missing value for --index-url}"
      shift 2
      ;;
    --registry-url)
      REGISTRY_URL="${2:?missing value for --registry-url}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "${PLUGIN_ID}" ]]; then
  echo "Missing --plugin <id>" >&2
  usage
  exit 1
fi

if ! ensure_rust_toolchain_path; then
  echo "Unable to locate cargo/rustup toolchain." >&2
  exit 1
fi

WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

PLUGIN_HOME="${WORK_DIR}/plugins"
TRUST_POLICY_PATH="${WORK_DIR}/trusted_publishers.json"
WORKSPACE="${WORK_DIR}/workspace"
mkdir -p "${PLUGIN_HOME}" "${WORKSPACE}"

install_args=(
  --plugin "${PLUGIN_ID}"
  --plugin-home "${PLUGIN_HOME}"
  --trust-policy-path "${TRUST_POLICY_PATH}"
)
if [[ -n "${REGISTRY_URL}" ]]; then
  install_args+=(--registry-url "${REGISTRY_URL}")
else
  install_args+=(--index-url "${INDEX_URL}")
fi

"${ROOT_DIR}/scripts/plugin-index-install.sh" "${install_args[@]}"

installed_json="$("${ROOT_DIR}/scripts/plugin-list.sh" --plugin-home "${PLUGIN_HOME}" --json)"
if ! jq -e --arg id "${PLUGIN_ID}" 'map(select(.id == $id)) | length > 0' <<< "${installed_json}" >/dev/null; then
  echo "Installed plugin '${PLUGIN_ID}' was not found in plugin home." >&2
  exit 1
fi

export KELVIN_PLUGIN_HOME="${PLUGIN_HOME}"
export KELVIN_TRUST_POLICY_PATH="${TRUST_POLICY_PATH}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target/plugin-compat}"

echo "[plugin-abi-compat] verifying '${PLUGIN_ID}' through kelvin-host runtime init"
cargo run -p kelvin-host -- \
  --prompt "ABI compatibility smoke test" \
  --workspace "${WORKSPACE}" \
  --memory fallback >/dev/null

echo "[plugin-abi-compat] success for '${PLUGIN_ID}'"
