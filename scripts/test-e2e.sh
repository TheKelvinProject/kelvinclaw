#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "${ROOT_DIR}"
echo "[test-e2e] running Kelvin brain/runtime e2e suites"
cargo test -p kelvin-brain --test core_contract_e2e
cargo test -p kelvin-brain --test installed_plugins_e2e
cargo test -p kelvin-brain --test mvp_secure_skill_e2e
echo "[test-e2e] running memory controller e2e/security acceptance suites"
cargo test -p kelvin-memory-controller --test memory_controller_owasp_top10_ai_2025
cargo test -p kelvin-memory-controller --test memory_controller_nist_ai_rmf_1_0
echo "[test-e2e] success"
