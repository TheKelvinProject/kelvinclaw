#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"
ensure_rust_toolchain_path || {
  echo "[kelvin-doctor] cargo/rustup not found" >&2
  exit 1
}

if [[ -x "${ROOT_DIR}/target/debug/kelvin-gateway" ]]; then
  exec "${ROOT_DIR}/target/debug/kelvin-gateway" --doctor "$@"
fi

exec cargo run -p kelvin-gateway -- --doctor "$@"
