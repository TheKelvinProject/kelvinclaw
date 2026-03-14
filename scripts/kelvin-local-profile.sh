#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"

PROFILE_DIR="${KELVIN_LOCAL_PROFILE_DIR:-${ROOT_DIR}/.kelvin/local-profile}"
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${ROOT_DIR}/.kelvin/plugins}"
TRUST_POLICY_PATH="${KELVIN_TRUST_POLICY_PATH:-${ROOT_DIR}/.kelvin/trusted_publishers.json}"
STATE_DIR="${KELVIN_STATE_DIR:-${ROOT_DIR}/.kelvin/state}"
GATEWAY_BIND="${KELVIN_LOCAL_GATEWAY_BIND:-127.0.0.1:34617}"
MEMORY_ADDR="${KELVIN_LOCAL_MEMORY_ADDR:-127.0.0.1:50051}"

MEMORY_PID_FILE="${PROFILE_DIR}/memory-controller.pid"
MEMORY_LOG_FILE="${PROFILE_DIR}/memory-controller.log"
GATEWAY_PID_FILE="${PROFILE_DIR}/gateway.pid"
GATEWAY_LOG_FILE="${PROFILE_DIR}/gateway.log"
MEMORY_BIN="${ROOT_DIR}/target/debug/kelvin-memory-controller"
GATEWAY_BIN="${ROOT_DIR}/target/debug/kelvin-gateway"

MEMORY_PUBLIC_KEY_PATH="${PROFILE_DIR}/memory-dev-public.pem"
MEMORY_PRIVATE_KEY_PATH="${PROFILE_DIR}/memory-dev-private.pem"
MEMORY_MODULE_MANIFEST="${KELVIN_MEMORY_MODULE_MANIFEST:-${ROOT_DIR}/crates/kelvin-memory-module-sdk/examples/memory_echo/manifest.json}"
MEMORY_MODULE_WAT="${KELVIN_MEMORY_MODULE_WAT:-${ROOT_DIR}/crates/kelvin-memory-module-sdk/examples/memory_echo/memory_echo.wat}"

usage() {
  cat <<'USAGE'
Usage: scripts/kelvin-local-profile.sh <command>

Run Kelvin's default local profile:
  - memory controller (data plane)
  - gateway (SDK runtime path)
  - installed plugins (CLI required, model optional)

Commands:
  start      Start memory controller + gateway with secure local defaults.
  stop       Stop both background processes.
  restart    Restart both processes.
  status     Show process status and endpoints.
  doctor     Run actionable gateway doctor output for this profile.
USAGE
}

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "[kelvin-local-profile] missing required command: ${name}" >&2
    exit 1
  fi
}

is_running() {
  local pid_file="$1"
  [[ -f "${pid_file}" ]] || return 1
  local pid
  pid="$(cat "${pid_file}")"
  [[ -n "${pid}" ]] && kill -0 "${pid}" >/dev/null 2>&1
}

is_loopback_bind() {
  local bind="$1"
  local host="${bind%:*}"
  [[ "${host}" == "127.0.0.1" || "${host}" == "::1" || "${host}" == "[::1]" || "${host}" == "localhost" ]]
}

validate_gateway_exposure() {
  if is_loopback_bind "${GATEWAY_BIND}"; then
    return 0
  fi

  if [[ -z "${KELVIN_GATEWAY_TOKEN:-}" ]]; then
    echo "[kelvin-local-profile] refusing public gateway bind without KELVIN_GATEWAY_TOKEN" >&2
    exit 1
  fi

  if [[ -n "${KELVIN_GATEWAY_TLS_CERT_PATH:-}" && -n "${KELVIN_GATEWAY_TLS_KEY_PATH:-}" ]]; then
    return 0
  fi

  case "${KELVIN_GATEWAY_ALLOW_INSECURE_PUBLIC_BIND:-0}" in
    1|true|TRUE|yes|YES|on|ON)
      echo "[kelvin-local-profile] warning: explicit insecure public bind override enabled" >&2
      return 0
      ;;
  esac

  echo "[kelvin-local-profile] refusing public gateway bind without TLS; set KELVIN_GATEWAY_TLS_CERT_PATH and KELVIN_GATEWAY_TLS_KEY_PATH or explicitly opt into KELVIN_GATEWAY_ALLOW_INSECURE_PUBLIC_BIND=1" >&2
  exit 1
}

write_memory_dev_keys() {
  mkdir -p "${PROFILE_DIR}"
  if [[ -f "${MEMORY_PRIVATE_KEY_PATH}" && -f "${MEMORY_PUBLIC_KEY_PATH}" ]]; then
    return 0
  fi
  umask 077
  openssl genpkey -algorithm ed25519 -out "${MEMORY_PRIVATE_KEY_PATH}" >/dev/null 2>&1
  openssl pkey -in "${MEMORY_PRIVATE_KEY_PATH}" -pubout -out "${MEMORY_PUBLIC_KEY_PATH}" >/dev/null 2>&1
}

ensure_prereqs() {
  ensure_rust_toolchain_path || {
    echo "[kelvin-local-profile] cargo/rustup not found" >&2
    exit 1
  }
  require_cmd cargo
  require_cmd jq
  require_cmd curl
  require_cmd tar
  require_cmd openssl

  mkdir -p "${PROFILE_DIR}" "${PLUGIN_HOME}" "$(dirname "${TRUST_POLICY_PATH}")" "${STATE_DIR}"
  write_memory_dev_keys
}

build_local_profile_binaries() {
  echo "[kelvin-local-profile] building local profile binaries"
  (cd "${ROOT_DIR}" && cargo build -p kelvin-memory-controller >/dev/null)
  (cd "${ROOT_DIR}" && cargo build -p kelvin-gateway --features memory_rpc >/dev/null)
  if [[ ! -x "${MEMORY_BIN}" ]]; then
    echo "[kelvin-local-profile] missing built memory controller binary: ${MEMORY_BIN}" >&2
    exit 1
  fi
  if [[ ! -x "${GATEWAY_BIN}" ]]; then
    echo "[kelvin-local-profile] missing built gateway binary: ${GATEWAY_BIN}" >&2
    exit 1
  fi
}

install_required_plugins() {
  if [[ -d "${PLUGIN_HOME}/kelvin.cli/current" ]]; then
    echo "[kelvin-local-profile] plugin already installed: kelvin.cli"
  else
    echo "[kelvin-local-profile] installing required plugin: kelvin.cli"
    "${ROOT_DIR}/scripts/install-kelvin-cli-plugin.sh" \
      --plugin-home "${PLUGIN_HOME}" \
      --trust-policy-path "${TRUST_POLICY_PATH}"
  fi

  if [[ -d "${PLUGIN_HOME}/kelvin.openai/current" ]]; then
    echo "[kelvin-local-profile] plugin already installed: kelvin.openai"
  else
    echo "[kelvin-local-profile] installing model plugin: kelvin.openai"
    "${ROOT_DIR}/scripts/install-kelvin-openai-plugin.sh" \
      --plugin-home "${PLUGIN_HOME}" \
      --trust-policy-path "${TRUST_POLICY_PATH}"
  fi
  if [[ -z "${OPENAI_API_KEY:-}" ]]; then
    echo "[kelvin-local-profile] OPENAI_API_KEY not set; gateway defaults to echo provider until key is configured"
  fi

  if [[ "${KELVIN_INSTALL_BROWSER_PLUGIN:-0}" == "1" ]]; then
    if [[ -d "${PLUGIN_HOME}/kelvin.browser.automation/current" ]]; then
      echo "[kelvin-local-profile] plugin already installed: kelvin.browser.automation"
    else
      echo "[kelvin-local-profile] installing optional plugin: kelvin.browser.automation"
      "${ROOT_DIR}/scripts/install-kelvin-browser-plugin.sh" \
        --plugin-home "${PLUGIN_HOME}" \
        --trust-policy-path "${TRUST_POLICY_PATH}"
    fi
  fi
}

start_memory_controller() {
  if is_running "${MEMORY_PID_FILE}"; then
    echo "[kelvin-local-profile] memory controller already running (pid $(cat "${MEMORY_PID_FILE}"))"
    return 0
  fi

  if [[ ! -f "${MEMORY_MODULE_MANIFEST}" ]]; then
    echo "[kelvin-local-profile] missing memory module manifest: ${MEMORY_MODULE_MANIFEST}" >&2
    exit 1
  fi
  if [[ ! -f "${MEMORY_MODULE_WAT}" ]]; then
    echo "[kelvin-local-profile] missing memory module wat: ${MEMORY_MODULE_WAT}" >&2
    exit 1
  fi

  (
    cd "${ROOT_DIR}"
    KELVIN_MEMORY_CONTROLLER_ADDR="${MEMORY_ADDR}" \
    KELVIN_MEMORY_PUBLIC_KEY_PATH="${MEMORY_PUBLIC_KEY_PATH}" \
    KELVIN_MEMORY_MODULE_MANIFEST="${MEMORY_MODULE_MANIFEST}" \
    KELVIN_MEMORY_MODULE_WAT="${MEMORY_MODULE_WAT}" \
      nohup "${MEMORY_BIN}" >>"${MEMORY_LOG_FILE}" 2>&1 &
    echo "$!" > "${MEMORY_PID_FILE}"
  )
  sleep 0.6
  if ! is_running "${MEMORY_PID_FILE}"; then
    echo "[kelvin-local-profile] memory controller failed to start; see ${MEMORY_LOG_FILE}" >&2
    rm -f "${MEMORY_PID_FILE}"
    exit 1
  fi
  echo "[kelvin-local-profile] memory controller started (pid $(cat "${MEMORY_PID_FILE}"))"
}

start_gateway() {
  if is_running "${GATEWAY_PID_FILE}"; then
    echo "[kelvin-local-profile] gateway already running (pid $(cat "${GATEWAY_PID_FILE}"))"
    return 0
  fi

  validate_gateway_exposure

  local -a gateway_args=(
    --bind "${GATEWAY_BIND}"
    --session "main"
    --workspace "${ROOT_DIR}"
    --state-dir "${STATE_DIR}"
    --persist-runs "true"
    --max-session-history "128"
    --compact-to "64"
    --require-cli-plugin "true"
  )
  if [[ -n "${KELVIN_GATEWAY_TOKEN:-}" ]]; then
    gateway_args+=(--token "${KELVIN_GATEWAY_TOKEN}")
  fi
  if [[ -n "${KELVIN_GATEWAY_TLS_CERT_PATH:-}" && -n "${KELVIN_GATEWAY_TLS_KEY_PATH:-}" ]]; then
    gateway_args+=(--tls-cert "${KELVIN_GATEWAY_TLS_CERT_PATH}" --tls-key "${KELVIN_GATEWAY_TLS_KEY_PATH}")
  fi
  case "${KELVIN_GATEWAY_ALLOW_INSECURE_PUBLIC_BIND:-0}" in
    1|true|TRUE|yes|YES|on|ON)
      gateway_args+=(--allow-insecure-public-bind true)
      ;;
  esac
  if [[ -n "${OPENAI_API_KEY:-}" ]]; then
    gateway_args+=(--model-provider "kelvin.openai")
  fi

  (
    cd "${ROOT_DIR}"
    KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" \
    KELVIN_TRUST_POLICY_PATH="${TRUST_POLICY_PATH}" \
    KELVIN_MEMORY_RPC_ENDPOINT="http://${MEMORY_ADDR}" \
    KELVIN_MEMORY_SIGNING_KEY_PATH="${MEMORY_PRIVATE_KEY_PATH}" \
    KELVIN_MEMORY_MODULE_ID="memory.echo" \
    KELVIN_MEMORY_WORKSPACE_ID="${ROOT_DIR}" \
    KELVIN_MEMORY_SESSION_ID="main" \
      nohup "${GATEWAY_BIN}" \
        "${gateway_args[@]}" >>"${GATEWAY_LOG_FILE}" 2>&1 &
    echo "$!" > "${GATEWAY_PID_FILE}"
  )
  sleep 0.6
  if ! is_running "${GATEWAY_PID_FILE}"; then
    echo "[kelvin-local-profile] gateway failed to start; see ${GATEWAY_LOG_FILE}" >&2
    rm -f "${GATEWAY_PID_FILE}"
    exit 1
  fi
  echo "[kelvin-local-profile] gateway started (pid $(cat "${GATEWAY_PID_FILE}"))"
}

stop_process() {
  local name="$1"
  local pid_file="$2"
  if ! is_running "${pid_file}"; then
    rm -f "${pid_file}"
    echo "[kelvin-local-profile] ${name} not running"
    return 0
  fi
  local pid
  pid="$(cat "${pid_file}")"
  kill "${pid}" >/dev/null 2>&1 || true
  for _ in {1..40}; do
    if ! kill -0 "${pid}" >/dev/null 2>&1; then
      rm -f "${pid_file}"
      echo "[kelvin-local-profile] ${name} stopped"
      return 0
    fi
    sleep 0.1
  done
  kill -9 "${pid}" >/dev/null 2>&1 || true
  rm -f "${pid_file}"
  echo "[kelvin-local-profile] ${name} force-stopped"
}

doctor_profile() {
  if [[ -n "${KELVIN_GATEWAY_TOKEN:-}" ]]; then
    "${ROOT_DIR}/scripts/kelvin-doctor.sh" \
      --endpoint "ws://${GATEWAY_BIND}" \
      --plugin-home "${PLUGIN_HOME}" \
      --trust-policy "${TRUST_POLICY_PATH}" \
      --token "${KELVIN_GATEWAY_TOKEN}"
    return 0
  fi
  "${ROOT_DIR}/scripts/kelvin-doctor.sh" \
    --endpoint "ws://${GATEWAY_BIND}" \
    --plugin-home "${PLUGIN_HOME}" \
    --trust-policy "${TRUST_POLICY_PATH}"
}

wait_until_ready() {
  for _ in {1..20}; do
    if doctor_profile >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  echo "[kelvin-local-profile] profile started but doctor check did not pass yet" >&2
  return 1
}

start_profile() {
  ensure_prereqs
  install_required_plugins
  build_local_profile_binaries
  start_memory_controller
  start_gateway
  wait_until_ready || true
  status_profile
  echo "[kelvin-local-profile] quick run:"
  echo "  scripts/try-kelvin.sh \"What is KelvinClaw?\""
  echo "[kelvin-local-profile] interactive mode:"
  echo "  source scripts/lib/rust-toolchain-path.sh && ensure_rust_toolchain_path && KELVIN_PLUGIN_HOME=\"${PLUGIN_HOME}\" KELVIN_TRUST_POLICY_PATH=\"${TRUST_POLICY_PATH}\" cargo run -p kelvin-host -- --interactive --workspace \"${ROOT_DIR}\" --state-dir \"${STATE_DIR}\""
}

stop_profile() {
  stop_process "gateway" "${GATEWAY_PID_FILE}"
  stop_process "memory controller" "${MEMORY_PID_FILE}"
}

status_profile() {
  if is_running "${MEMORY_PID_FILE}"; then
    echo "[kelvin-local-profile] memory controller: running (pid $(cat "${MEMORY_PID_FILE}")) addr=${MEMORY_ADDR}"
  else
    echo "[kelvin-local-profile] memory controller: stopped"
  fi
  if is_running "${GATEWAY_PID_FILE}"; then
    echo "[kelvin-local-profile] gateway: running (pid $(cat "${GATEWAY_PID_FILE}")) ws=ws://${GATEWAY_BIND}"
  else
    echo "[kelvin-local-profile] gateway: stopped"
  fi
  echo "[kelvin-local-profile] plugin_home=${PLUGIN_HOME}"
  echo "[kelvin-local-profile] trust_policy=${TRUST_POLICY_PATH}"
  echo "[kelvin-local-profile] state_dir=${STATE_DIR}"
  echo "[kelvin-local-profile] logs: ${MEMORY_LOG_FILE} | ${GATEWAY_LOG_FILE}"
}

main() {
  if [[ $# -lt 1 ]]; then
    usage
    exit 1
  fi

  local command="$1"
  shift || true

  case "${command}" in
    start)
      start_profile "$@"
      ;;
    stop)
      stop_profile
      ;;
    restart)
      stop_profile
      start_profile "$@"
      ;;
    status)
      status_profile
      ;;
    doctor)
      doctor_profile
      ;;
    *)
      usage
      exit 1
      ;;
  esac
}

main "$@"
