#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"

MODE="local" # local | docker
PROMPT="${KELVIN_QUICKSTART_PROMPT:-What is KelvinClaw?}"

usage() {
  cat <<'USAGE'
Usage: scripts/quickstart.sh [options]

Canonical quick start for KelvinClaw Daily Driver MVP.

Options:
  --mode <local|docker>  Run local profile or runtime container flow (default: local)
  --prompt <text>        Prompt used for local smoke run
  -h, --help             Show help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)
      MODE="${2:?missing value for --mode}"
      shift 2
      ;;
    --prompt)
      PROMPT="${2:?missing value for --prompt}"
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

if [[ "${MODE}" == "docker" ]]; then
  echo "[quickstart] mode=docker"
  exec "${ROOT_DIR}/scripts/run-runtime-container.sh"
fi

if [[ "${MODE}" != "local" ]]; then
  echo "Invalid mode: ${MODE} (expected local or docker)" >&2
  exit 1
fi

echo "[quickstart] mode=local"
"${ROOT_DIR}/scripts/kelvin-local-profile.sh" start

ensure_rust_toolchain_path || {
  echo "[quickstart] cargo/rustup required for local host run" >&2
  exit 1
}

PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${ROOT_DIR}/.kelvin/plugins}"
TRUST_POLICY_PATH="${KELVIN_TRUST_POLICY_PATH:-${ROOT_DIR}/.kelvin/trusted_publishers.json}"
STATE_DIR="${KELVIN_STATE_DIR:-${ROOT_DIR}/.kelvin/state}"

if [[ -n "${OPENAI_API_KEY:-}" ]]; then
  KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" \
  KELVIN_TRUST_POLICY_PATH="${TRUST_POLICY_PATH}" \
    cargo run -p kelvin-host -- \
      --prompt "${PROMPT}" \
      --workspace "${ROOT_DIR}" \
      --state-dir "${STATE_DIR}" \
      --model-provider "kelvin.openai"
else
  KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" \
  KELVIN_TRUST_POLICY_PATH="${TRUST_POLICY_PATH}" \
    cargo run -p kelvin-host -- \
      --prompt "${PROMPT}" \
      --workspace "${ROOT_DIR}" \
      --state-dir "${STATE_DIR}"
fi

echo "[quickstart] success"
