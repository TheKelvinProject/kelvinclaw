#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"

if ! ensure_rust_toolchain_path; then
  echo "[test-contracts] missing required commands: cargo/rustup" >&2
  exit 1
fi

cd "${ROOT_DIR}"

echo "[test-contracts] memory rpc descriptor and context contract"
cargo test -p kelvin-memory-api descriptor_contract

echo "[test-contracts] gateway protocol compatibility contract"
cargo test -p kelvin-gateway gateway_protocol_version_is_stable
cargo test -p kelvin-gateway gateway_method_contract_matches_v1_surface
cargo test -p kelvin-gateway gateway_rejects_unknown_method_with_method_not_found

echo "[test-contracts] wasm model ABI contract"
cargo test -p kelvin-wasm abi_constants_are_stable
cargo test -p kelvin-wasm model_host_rejects_unsupported_import_module

echo "[test-contracts] success"
