#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DAEMON_DIR="${KELVIN_GATEWAY_DAEMON_DIR:-${ROOT_DIR}/.kelvin/gateway-daemon}"
PID_FILE="${DAEMON_DIR}/kelvin-gateway.pid"
LOG_FILE="${DAEMON_DIR}/kelvin-gateway.log"
BIN_PATH="${ROOT_DIR}/target/debug/kelvin-gateway"

source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"

usage() {
  cat <<'EOF'
Usage: scripts/kelvin-gateway-daemon.sh <command> [-- gateway_args...]

Commands:
  start      Build (if needed) and start kelvin-gateway in background.
  stop       Stop background gateway process.
  restart    Restart daemon.
  status     Show daemon process status.
  logs       Tail daemon log file.
  health     Run gateway doctor against daemon endpoint.
EOF
}

is_running() {
  if [[ ! -f "${PID_FILE}" ]]; then
    return 1
  fi
  local pid
  pid="$(cat "${PID_FILE}")"
  [[ -n "${pid}" ]] && kill -0 "${pid}" >/dev/null 2>&1
}

build_binary_if_needed() {
  ensure_rust_toolchain_path || {
    echo "[gateway-daemon] cargo/rustup not found" >&2
    exit 1
  }
  if [[ ! -x "${BIN_PATH}" ]]; then
    (cd "${ROOT_DIR}" && cargo build -p kelvin-gateway >/dev/null)
  fi
}

start_daemon() {
  mkdir -p "${DAEMON_DIR}"
  if is_running; then
    echo "[gateway-daemon] already running (pid $(cat "${PID_FILE}"))"
    return 0
  fi

  build_binary_if_needed
  local gateway_args=("$@")
  if [[ "${#gateway_args[@]}" -eq 0 ]]; then
    gateway_args=(--bind 127.0.0.1:34617)
  fi
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

  (
    cd "${ROOT_DIR}"
    nohup "${BIN_PATH}" "${gateway_args[@]}" >>"${LOG_FILE}" 2>&1 &
    echo "$!" >"${PID_FILE}"
  )
  sleep 0.5
  if is_running; then
    echo "[gateway-daemon] started (pid $(cat "${PID_FILE}"))"
    echo "[gateway-daemon] log: ${LOG_FILE}"
  else
    echo "[gateway-daemon] failed to start; check ${LOG_FILE}" >&2
    rm -f "${PID_FILE}"
    exit 1
  fi
}

stop_daemon() {
  if ! is_running; then
    echo "[gateway-daemon] not running"
    rm -f "${PID_FILE}"
    return 0
  fi
  local pid
  pid="$(cat "${PID_FILE}")"
  kill "${pid}" >/dev/null 2>&1 || true
  for _ in {1..40}; do
    if ! kill -0 "${pid}" >/dev/null 2>&1; then
      rm -f "${PID_FILE}"
      echo "[gateway-daemon] stopped"
      return 0
    fi
    sleep 0.1
  done
  echo "[gateway-daemon] force killing pid ${pid}"
  kill -9 "${pid}" >/dev/null 2>&1 || true
  rm -f "${PID_FILE}"
}

status_daemon() {
  if is_running; then
    echo "[gateway-daemon] running (pid $(cat "${PID_FILE}"))"
  else
    echo "[gateway-daemon] stopped"
    return 1
  fi
}

health_daemon() {
  build_binary_if_needed
  local token_arg=()
  if [[ -n "${KELVIN_GATEWAY_TOKEN:-}" ]]; then
    token_arg=(--token "${KELVIN_GATEWAY_TOKEN}")
  fi
  (cd "${ROOT_DIR}" && "${BIN_PATH}" --doctor "${token_arg[@]}")
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
      if [[ "${1:-}" == "--" ]]; then
        shift
      fi
      start_daemon "$@"
      ;;
    stop)
      stop_daemon
      ;;
    restart)
      stop_daemon
      if [[ "${1:-}" == "--" ]]; then
        shift
      fi
      start_daemon "$@"
      ;;
    status)
      status_daemon
      ;;
    logs)
      mkdir -p "${DAEMON_DIR}"
      touch "${LOG_FILE}"
      tail -n 200 -f "${LOG_FILE}"
      ;;
    health)
      health_daemon
      ;;
    *)
      usage
      exit 1
      ;;
  esac
}

main "$@"
