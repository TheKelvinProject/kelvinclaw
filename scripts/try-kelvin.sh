#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"
PROMPT="${1:-hello kelvin}"
TIMEOUT_MS="${KELVIN_TRY_TIMEOUT_MS:-5000}"
MODE="${KELVIN_TRY_MODE:-auto}" # auto | local | docker
TARGET_DIR="${KELVIN_TRY_TARGET_DIR:-${ROOT_DIR}/target}"
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${ROOT_DIR}/.kelvin/plugins}"
TRUST_POLICY_PATH="${KELVIN_TRUST_POLICY_PATH:-${ROOT_DIR}/.kelvin/trusted_publishers.json}"
DOCKER_IMAGE="${KELVIN_TRY_DOCKER_IMAGE:-rust:1.93.1-bookworm}"
PLUGIN_INDEX_URL="${KELVIN_PLUGIN_INDEX_URL:-https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json}"

ensure_cli_plugin() {
  if [[ -d "${PLUGIN_HOME}/kelvin.cli/current" ]]; then
    echo "[try-kelvin] kelvin_cli plugin already installed: ${PLUGIN_HOME}/kelvin.cli/current"
    return 0
  fi
  echo "[try-kelvin] ensuring kelvin_cli WASM plugin is installed"
  KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" \
  KELVIN_TRUST_POLICY_PATH="${TRUST_POLICY_PATH}" \
    "${ROOT_DIR}/scripts/install-kelvin-cli-plugin.sh" \
      --plugin-home "${PLUGIN_HOME}" \
      --trust-policy-path "${TRUST_POLICY_PATH}"
}

ensure_cli_plugin_docker() {
  echo "[try-kelvin] ensuring kelvin_cli plugin is installed (docker bootstrap)"
  docker run --rm \
    -e DEBIAN_FRONTEND=noninteractive \
    -e KELVIN_PLUGIN_HOME="/work/.kelvin/plugins" \
    -e KELVIN_TRUST_POLICY_PATH="/work/.kelvin/trusted_publishers.json" \
    -e KELVIN_PLUGIN_INDEX_URL="${PLUGIN_INDEX_URL}" \
    -v "${ROOT_DIR}:/work" \
    -w /work \
    "${DOCKER_IMAGE}" \
    bash -lc '
      set -euo pipefail
      if [[ -d "$KELVIN_PLUGIN_HOME/kelvin.cli/current" ]]; then
        echo "[try-kelvin] kelvin_cli plugin already installed: $KELVIN_PLUGIN_HOME/kelvin.cli/current"
        exit 0
      fi
      if ! command -v jq >/dev/null 2>&1; then
        apt-get update -qq >/dev/null
        apt-get install -y --no-install-recommends jq >/dev/null
      fi
      scripts/install-kelvin-cli-plugin.sh \
        --plugin-home "$KELVIN_PLUGIN_HOME" \
        --trust-policy-path "$KELVIN_TRUST_POLICY_PATH"
    '
}

run_local() {
  echo "[try-kelvin] mode=local"
  ensure_cli_plugin
  cd "${ROOT_DIR}"
  KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" \
  KELVIN_TRUST_POLICY_PATH="${TRUST_POLICY_PATH}" \
  KELVIN_PLUGIN_INDEX_URL="${PLUGIN_INDEX_URL}" \
  CARGO_TARGET_DIR="${TARGET_DIR}" \
    cargo run -p kelvin-host -- \
      --prompt "${PROMPT}" \
      --timeout-ms "${TIMEOUT_MS}"
}

run_docker() {
  echo "[try-kelvin] mode=docker"
  ensure_cli_plugin_docker
  docker run --rm \
    -e KELVIN_TRY_PROMPT="${PROMPT}" \
    -e KELVIN_TRY_TIMEOUT_MS="${TIMEOUT_MS}" \
    -e KELVIN_TRY_TARGET_DIR="/work/target/try-kelvin-cli" \
    -e KELVIN_PLUGIN_HOME="/work/.kelvin/plugins" \
    -e KELVIN_TRUST_POLICY_PATH="/work/.kelvin/trusted_publishers.json" \
    -e KELVIN_PLUGIN_INDEX_URL="${PLUGIN_INDEX_URL}" \
    -v "${ROOT_DIR}:/work" \
    -w /work \
    "${DOCKER_IMAGE}" \
    bash -lc 'export PATH=/usr/local/cargo/bin:$PATH && CARGO_TARGET_DIR="$KELVIN_TRY_TARGET_DIR" cargo run -p kelvin-host -- --prompt "$KELVIN_TRY_PROMPT" --timeout-ms "$KELVIN_TRY_TIMEOUT_MS"'
}

if [[ "${MODE}" == "local" ]]; then
  if ! ensure_rust_toolchain_path || ! command -v cargo >/dev/null 2>&1; then
    echo "[try-kelvin] error: cargo not found and KELVIN_TRY_MODE=local was requested" >&2
    exit 1
  fi
  run_local
  exit 0
fi

if [[ "${MODE}" == "docker" ]]; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "[try-kelvin] error: docker not found and KELVIN_TRY_MODE=docker was requested" >&2
    exit 1
  fi
  run_docker
  exit 0
fi

if ensure_rust_toolchain_path && command -v cargo >/dev/null 2>&1; then
  run_local
elif command -v docker >/dev/null 2>&1; then
  run_docker
else
  echo "[try-kelvin] error: neither cargo nor docker is available" >&2
  exit 1
fi
