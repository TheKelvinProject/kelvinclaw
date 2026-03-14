#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"

RUNNER_PATH="${ROOT_DIR}/scripts/kelvin-gateway-service-run.sh"
ENV_FILE="${ROOT_DIR}/.env"
BINARY_PATH="${ROOT_DIR}/target/debug/kelvin-gateway"
WORKSPACE_DIR="${ROOT_DIR}"
STATE_DIR="${ROOT_DIR}/.kelvin/gateway-state"
LOG_DIR="${ROOT_DIR}/.kelvin/gateway-daemon"
BIND_ADDR="127.0.0.1:34617"
INGRESS_BIND="127.0.0.1:34618"
SYSTEMD_UNIT_NAME="kelvin-gateway"
LAUNCHD_LABEL="dev.kelvinclaw.gateway"
OUTPUT_PATH=""

usage() {
  cat <<'USAGE'
Usage:
  scripts/kelvin-gateway-service.sh render-systemd-user [options]
  scripts/kelvin-gateway-service.sh install-systemd-user [options]
  scripts/kelvin-gateway-service.sh render-launchd [options]
  scripts/kelvin-gateway-service.sh install-launchd [options]

Render or install user-level service definitions for kelvin-gateway.

Options:
  --env-file <path>        Environment file sourced by the service runner
  --binary <path>          kelvin-gateway binary path
  --workspace <path>       Default workspace passed to kelvin-gateway
  --state-dir <path>       Persisted gateway state directory
  --log-dir <path>         Launchd stdout/stderr log directory
  --bind <host:port>       WebSocket bind address
  --ingress-bind <addr>    HTTP ingress/operator bind address (empty disables)
  --systemd-unit <name>    systemd user unit name without suffix (default: kelvin-gateway)
  --launchd-label <label>  launchd label (default: dev.kelvinclaw.gateway)
  --output <path>          Write rendered output to an explicit file
  -h, --help               Show help
USAGE
}

render_systemd_user() {
  cat <<EOF
[Unit]
Description=KelvinClaw Gateway
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=${WORKSPACE_DIR}
Environment=KELVIN_GATEWAY_SERVICE_ENV_FILE=${ENV_FILE}
Environment=KELVIN_GATEWAY_SERVICE_BINARY=${BINARY_PATH}
Environment=KELVIN_GATEWAY_WORKSPACE=${WORKSPACE_DIR}
Environment=KELVIN_GATEWAY_STATE_DIR=${STATE_DIR}
Environment=KELVIN_GATEWAY_BIND=${BIND_ADDR}
Environment=KELVIN_GATEWAY_INGRESS_BIND=${INGRESS_BIND}
ExecStart=${RUNNER_PATH}
Restart=on-failure
RestartSec=2
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=default.target
EOF
}

render_launchd() {
  mkdir -p "${LOG_DIR}"
  cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${RUNNER_PATH}</string>
  </array>
  <key>WorkingDirectory</key>
  <string>${WORKSPACE_DIR}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>KELVIN_GATEWAY_SERVICE_ENV_FILE</key>
    <string>${ENV_FILE}</string>
    <key>KELVIN_GATEWAY_SERVICE_BINARY</key>
    <string>${BINARY_PATH}</string>
    <key>KELVIN_GATEWAY_WORKSPACE</key>
    <string>${WORKSPACE_DIR}</string>
    <key>KELVIN_GATEWAY_STATE_DIR</key>
    <string>${STATE_DIR}</string>
    <key>KELVIN_GATEWAY_BIND</key>
    <string>${BIND_ADDR}</string>
    <key>KELVIN_GATEWAY_INGRESS_BIND</key>
    <string>${INGRESS_BIND}</string>
  </dict>
  <key>KeepAlive</key>
  <true/>
  <key>RunAtLoad</key>
  <true/>
  <key>StandardOutPath</key>
  <string>${LOG_DIR}/kelvin-gateway.out.log</string>
  <key>StandardErrorPath</key>
  <string>${LOG_DIR}/kelvin-gateway.err.log</string>
</dict>
</plist>
EOF
}

ensure_binary() {
  if [[ -x "${BINARY_PATH}" ]]; then
    return 0
  fi
  ensure_rust_toolchain_path || {
    echo "[gateway-service] missing kelvin-gateway binary at ${BINARY_PATH} and cargo/rustup is unavailable" >&2
    exit 1
  }
  (cd "${ROOT_DIR}" && cargo build -p kelvin-gateway >/dev/null)
}

write_output() {
  local target="$1"
  local renderer="$2"
  mkdir -p "$(dirname "${target}")"
  "${renderer}" > "${target}"
  echo "${target}"
}

if [[ $# -lt 1 ]]; then
  usage
  exit 1
fi

COMMAND="$1"
shift

while [[ $# -gt 0 ]]; do
  case "$1" in
    --env-file)
      ENV_FILE="${2:?missing value for --env-file}"
      shift 2
      ;;
    --binary)
      BINARY_PATH="${2:?missing value for --binary}"
      shift 2
      ;;
    --workspace)
      WORKSPACE_DIR="${2:?missing value for --workspace}"
      shift 2
      ;;
    --state-dir)
      STATE_DIR="${2:?missing value for --state-dir}"
      shift 2
      ;;
    --log-dir)
      LOG_DIR="${2:?missing value for --log-dir}"
      shift 2
      ;;
    --bind)
      BIND_ADDR="${2:?missing value for --bind}"
      shift 2
      ;;
    --ingress-bind)
      INGRESS_BIND="${2:?missing value for --ingress-bind}"
      shift 2
      ;;
    --systemd-unit)
      SYSTEMD_UNIT_NAME="${2:?missing value for --systemd-unit}"
      shift 2
      ;;
    --launchd-label)
      LAUNCHD_LABEL="${2:?missing value for --launchd-label}"
      shift 2
      ;;
    --output)
      OUTPUT_PATH="${2:?missing value for --output}"
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

case "${COMMAND}" in
  render-systemd-user)
    render_systemd_user
    ;;
  install-systemd-user)
    ensure_binary
    target="${OUTPUT_PATH:-${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user/${SYSTEMD_UNIT_NAME}.service}"
    target="$(write_output "${target}" render_systemd_user)"
    echo "[gateway-service] wrote ${target}"
    echo "[gateway-service] next: systemctl --user daemon-reload && systemctl --user enable --now ${SYSTEMD_UNIT_NAME}.service"
    ;;
  render-launchd)
    render_launchd
    ;;
  install-launchd)
    ensure_binary
    target="${OUTPUT_PATH:-${HOME}/Library/LaunchAgents/${LAUNCHD_LABEL}.plist}"
    target="$(write_output "${target}" render_launchd)"
    echo "[gateway-service] wrote ${target}"
    echo "[gateway-service] next: launchctl bootout gui/$(id -u) ${LAUNCHD_LABEL} 2>/dev/null || true"
    echo "[gateway-service] next: launchctl bootstrap gui/$(id -u) ${target}"
    ;;
  *)
    usage
    exit 1
    ;;
esac
