#!/usr/bin/env bash
set -euo pipefail

HOST="${REMOTE_TEST_HOST:-}"
REMOTE_DIR="~/kelvinclaw"
MODE="native"
DO_SYNC="1"
EXTRA_CARGO_ARGS=""
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Load local .env defaults for convenience, but do not override an already-set
# shell environment variable.
if [[ -z "${REMOTE_TEST_HOST:-}" && -f "${ROOT_DIR}/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "${ROOT_DIR}/.env"
  set +a
fi

HOST="${HOST:-${REMOTE_TEST_HOST:-}}"

usage() {
  cat <<USAGE
Usage: scripts/remote-test.sh [options]

Sync this repository to a remote host over SSH and run Rust tests there.
If REMOTE_TEST_HOST is not set in your shell, the script will read .env from
the repository root.

Options:
  --host <host>            SSH host (default: \$REMOTE_TEST_HOST)
  --remote-dir <path>      Remote project dir (default: ${REMOTE_DIR})
  --mode <native|docker>   Test mode (default: ${MODE})
  --docker                 Shortcut for --mode docker
  --native                 Shortcut for --mode native
  --no-sync                Skip file sync step
  --cargo-args '<args>'    Extra args appended to cargo test
  -h, --help               Show this help

Examples:
  REMOTE_TEST_HOST=your-host scripts/remote-test.sh
  scripts/remote-test.sh --docker
  scripts/remote-test.sh --host ec2-user@your-host --cargo-args '-- --nocapture'
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --host)
      HOST="${2:?missing value for --host}"
      shift 2
      ;;
    --remote-dir)
      REMOTE_DIR="${2:?missing value for --remote-dir}"
      shift 2
      ;;
    --mode)
      MODE="${2:?missing value for --mode}"
      shift 2
      ;;
    --docker)
      MODE="docker"
      shift
      ;;
    --native)
      MODE="native"
      shift
      ;;
    --no-sync)
      DO_SYNC="0"
      shift
      ;;
    --cargo-args)
      EXTRA_CARGO_ARGS="${2:?missing value for --cargo-args}"
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

if [[ "${MODE}" != "native" && "${MODE}" != "docker" ]]; then
  echo "Invalid mode: ${MODE} (expected native or docker)" >&2
  exit 1
fi

if [[ -z "${HOST}" ]]; then
  echo "Missing remote host. Pass --host <host> or set REMOTE_TEST_HOST." >&2
  exit 1
fi

echo "[remote-test] host=${HOST} mode=${MODE} remote_dir=${REMOTE_DIR}"

echo "[remote-test] checking SSH connectivity"
ssh -o BatchMode=yes -o ConnectTimeout=8 "${HOST}" 'echo ok >/dev/null'

if [[ "${MODE}" == "docker" ]]; then
  echo "[remote-test] checking remote Docker access"
  if ! ssh "${HOST}" "docker info >/dev/null 2>&1"; then
    cat >&2 <<MSG
Remote Docker mode requested, but the current SSH user cannot access the Docker daemon.
Use one of:
  1) scripts/remote-test.sh --native
  2) grant this remote user Docker daemon access (for example docker group membership)
MSG
    exit 1
  fi
fi

if [[ "${DO_SYNC}" == "1" ]]; then
  echo "[remote-test] syncing repository"
  if command -v rsync >/dev/null 2>&1; then
    rsync -az --delete \
      --exclude '.git' \
      --exclude 'target' \
      --exclude '.DS_Store' \
      "${ROOT_DIR}/" "${HOST}:${REMOTE_DIR}/"
  else
    tar czf - \
      --exclude='.git' \
      --exclude='target' \
      --exclude='.DS_Store' \
      -C "${ROOT_DIR}" . | ssh "${HOST}" "rm -rf ${REMOTE_DIR} && mkdir -p ${REMOTE_DIR} && tar xzf - -C ${REMOTE_DIR}"
  fi
fi

if [[ "${MODE}" == "native" ]]; then
  echo "[remote-test] running native cargo test"
  ssh "${HOST}" "source \$HOME/.cargo/env && cd ${REMOTE_DIR} && cargo test --workspace ${EXTRA_CARGO_ARGS}"
else
  echo "[remote-test] running cargo test in Docker"
  ssh "${HOST}" "REMOTE_DIR='${REMOTE_DIR}'; REMOTE_DIR=\${REMOTE_DIR/#\~/\$HOME}; cd \${REMOTE_DIR} && docker run --rm -v \"\${REMOTE_DIR}:/work\" -w /work rust:1.77 cargo test --workspace ${EXTRA_CARGO_ARGS}"
fi

echo "[remote-test] done"
