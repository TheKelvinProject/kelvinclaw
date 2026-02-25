#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "${ROOT_DIR}"
echo "[test-sdk] running Kelvin Core SDK tests"
cargo test -p kelvin-core --test sdk_security_stability
cargo test -p kelvin-core --test sdk_owasp_top10_ai_2025
cargo test -p kelvin-core --test sdk_nist_ai_rmf_1_0
cargo test -p kelvin-core sdk::tests
echo "[test-sdk] running SDK model-provider lane tests"
cargo test -p kelvin-wasm model_host::tests
cargo test -p kelvin-brain installed_plugins::tests
cargo test -p kelvin-sdk --lib
echo "[test-sdk] success"
