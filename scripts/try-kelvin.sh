#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROMPT="${1:-hello kelvin}"
TIMEOUT_MS="${KELVIN_TRY_TIMEOUT_MS:-5000}"
MODE="${KELVIN_TRY_MODE:-auto}" # auto | local | docker
TARGET_DIR="${KELVIN_TRY_TARGET_DIR:-${ROOT_DIR}/target/try-kelvin-cli}"

run_local() {
  echo "[try-kelvin] mode=local"
  cd "${ROOT_DIR}"
  CARGO_TARGET_DIR="${TARGET_DIR}" cargo run --manifest-path archive/kelvin-cli/Cargo.toml -- \
    --prompt "${PROMPT}" \
    --timeout-ms "${TIMEOUT_MS}"
}

run_docker() {
  echo "[try-kelvin] mode=docker"
  docker run --rm \
    -e KELVIN_TRY_PROMPT="${PROMPT}" \
    -e KELVIN_TRY_TIMEOUT_MS="${TIMEOUT_MS}" \
    -e KELVIN_TRY_TARGET_DIR="/work/target/try-kelvin-cli" \
    -v "${ROOT_DIR}:/work" \
    -w /work \
    rust:latest \
    bash -lc 'export PATH=/usr/local/cargo/bin:$PATH && CARGO_TARGET_DIR="$KELVIN_TRY_TARGET_DIR" cargo run --manifest-path archive/kelvin-cli/Cargo.toml -- --prompt "$KELVIN_TRY_PROMPT" --timeout-ms "$KELVIN_TRY_TIMEOUT_MS"'
}

if [[ "${MODE}" == "local" ]]; then
  if ! command -v cargo >/dev/null 2>&1; then
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

if command -v cargo >/dev/null 2>&1; then
  run_local
elif command -v docker >/dev/null 2>&1; then
  run_docker
else
  echo "[try-kelvin] error: neither cargo nor docker is available" >&2
  exit 1
fi
