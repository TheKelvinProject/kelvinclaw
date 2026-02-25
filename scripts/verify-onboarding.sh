#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRACK="all"
PROMPT_BASE="${KELVIN_VERIFY_PROMPT:-kelvin onboarding check}"
TARGET_DIR="${KELVIN_VERIFY_TARGET_DIR:-${ROOT_DIR}/target/verify-onboarding}"

usage() {
  cat <<'USAGE'
Usage: scripts/verify-onboarding.sh [options]

Verify onboarding instructions by user skill track.

Tracks:
  beginner  Docker-only path (no local Rust required)
  rust      Local Rust runtime path
  wasm      Rust + wasm plugin-author path
  all       Run all tracks (default)

Options:
  --track <beginner|rust|wasm|all>  Select verification track
  --prompt <text>                   Prompt used for runtime smoke checks
  -h, --help                        Show help

Examples:
  scripts/verify-onboarding.sh --track beginner
  scripts/verify-onboarding.sh --track rust
  scripts/verify-onboarding.sh --track wasm
  scripts/verify-onboarding.sh --track all
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --track)
      TRACK="${2:?missing value for --track}"
      shift 2
      ;;
    --prompt)
      PROMPT_BASE="${2:?missing value for --prompt}"
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

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "[verify-onboarding] missing required command: ${name}" >&2
    exit 1
  fi
}

assert_echo_payload() {
  local log_path="$1"
  local track_name="$2"
  if ! grep -q "payload: Echo:" "${log_path}"; then
    echo "[verify-onboarding] ${track_name}: expected echo payload not found" >&2
    sed -n '1,200p' "${log_path}" >&2
    exit 1
  fi
}

run_beginner() {
  echo "[verify-onboarding] track=beginner"
  require_cmd docker
  local log_path
  log_path="$(mktemp)"
  (
    cd "${ROOT_DIR}"
    KELVIN_TRY_MODE=docker \
    KELVIN_TRY_TARGET_DIR="${TARGET_DIR}/beginner" \
      scripts/try-kelvin.sh "${PROMPT_BASE} beginner"
  ) | tee "${log_path}"
  assert_echo_payload "${log_path}" "beginner"
  rm -f "${log_path}"
  echo "[verify-onboarding] track=beginner result=pass"
}

run_rust() {
  echo "[verify-onboarding] track=rust"
  require_cmd cargo
  (
    cd "${ROOT_DIR}"
    scripts/test-sdk.sh
  )
  local log_path
  log_path="$(mktemp)"
  (
    cd "${ROOT_DIR}"
    KELVIN_TRY_MODE=local \
    KELVIN_TRY_TARGET_DIR="${TARGET_DIR}/rust" \
      scripts/try-kelvin.sh "${PROMPT_BASE} rust"
  ) | tee "${log_path}"
  assert_echo_payload "${log_path}" "rust"
  rm -f "${log_path}"
  echo "[verify-onboarding] track=rust result=pass"
}

run_wasm() {
  echo "[verify-onboarding] track=wasm"
  require_cmd cargo
  require_cmd rustup
  if ! rustup target list --installed | grep -qx 'wasm32-unknown-unknown'; then
    cat >&2 <<'MSG'
[verify-onboarding] track=wasm missing rust target: wasm32-unknown-unknown
Install with:
  rustup target add wasm32-unknown-unknown
MSG
    exit 1
  fi

  (
    cd "${ROOT_DIR}"
    CARGO_TARGET_DIR="${TARGET_DIR}/wasm-build" \
      cargo build --target wasm32-unknown-unknown --manifest-path examples/echo-wasm-skill/Cargo.toml
  )

  local wasm_path="${TARGET_DIR}/wasm-build/wasm32-unknown-unknown/debug/echo_wasm_skill.wasm"
  if [[ ! -f "${wasm_path}" ]]; then
    echo "[verify-onboarding] track=wasm expected build artifact not found: ${wasm_path}" >&2
    exit 1
  fi

  (
    cd "${ROOT_DIR}"
    CARGO_TARGET_DIR="${TARGET_DIR}/wasm-runner" \
      cargo run -p kelvin-wasm --bin kelvin-wasm-runner -- --wasm "${wasm_path}" --policy-preset locked_down
  )

  echo "[verify-onboarding] track=wasm result=pass"
}

case "${TRACK}" in
  beginner)
    run_beginner
    ;;
  rust)
    run_rust
    ;;
  wasm)
    run_wasm
    ;;
  all)
    run_beginner
    run_rust
    run_wasm
    ;;
  *)
    echo "Invalid track: ${TRACK} (expected beginner, rust, wasm, or all)" >&2
    exit 1
    ;;
esac

echo "[verify-onboarding] complete"
