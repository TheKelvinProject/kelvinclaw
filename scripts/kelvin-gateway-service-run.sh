#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"

ENV_FILE="${KELVIN_GATEWAY_SERVICE_ENV_FILE:-${ROOT_DIR}/.env}"
if [[ -f "${ENV_FILE}" ]]; then
  set -a
  source "${ENV_FILE}"
  set +a
fi

BIN_PATH="${KELVIN_GATEWAY_SERVICE_BINARY:-${ROOT_DIR}/target/debug/kelvin-gateway}"
WORKSPACE_DIR="${KELVIN_GATEWAY_WORKSPACE:-${ROOT_DIR}}"
STATE_DIR="${KELVIN_GATEWAY_STATE_DIR:-${ROOT_DIR}/.kelvin/gateway-state}"
BIND_ADDR="${KELVIN_GATEWAY_BIND:-127.0.0.1:34617}"
INGRESS_BIND="${KELVIN_GATEWAY_INGRESS_BIND:-127.0.0.1:34618}"
LOAD_INSTALLED_PLUGINS="${KELVIN_GATEWAY_LOAD_INSTALLED_PLUGINS:-true}"
PERSIST_RUNS="${KELVIN_GATEWAY_PERSIST_RUNS:-true}"

if [[ ! -x "${BIN_PATH}" ]]; then
  ensure_rust_toolchain_path || {
    echo "[gateway-service-run] missing kelvin-gateway binary at ${BIN_PATH} and cargo/rustup is unavailable" >&2
    exit 1
  }
  (cd "${ROOT_DIR}" && cargo build -p kelvin-gateway >/dev/null)
fi

mkdir -p "${STATE_DIR}"

args=(
  --bind "${BIND_ADDR}"
  --workspace "${WORKSPACE_DIR}"
  --state-dir "${STATE_DIR}"
  --persist-runs "${PERSIST_RUNS}"
  --load-installed-plugins "${LOAD_INSTALLED_PLUGINS}"
)

if [[ -n "${INGRESS_BIND}" ]]; then
  args+=(--ingress-bind "${INGRESS_BIND}")
fi

exec "${BIN_PATH}" "${args[@]}" "$@"
