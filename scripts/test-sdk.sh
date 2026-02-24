#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "${ROOT_DIR}"
echo "[test-sdk] running Kelvin Core SDK tests"
cargo test -p kelvin-core --test sdk_security_stability
cargo test -p kelvin-core --test sdk_owasp_top10_ai_2025
cargo test -p kelvin-core sdk::tests
echo "[test-sdk] success"
