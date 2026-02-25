#!/usr/bin/env bash
set -euo pipefail

HOST="${REMOTE_TEST_HOST:-}"
REMOTE_DIR="${REMOTE_TEST_REMOTE_DIR:-~/kelvinclaw}"
MODE="native"
DO_SYNC="1"
EXTRA_CARGO_ARGS=""
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

trim_whitespace() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "${value}"
}

strip_wrapping_quotes() {
  local value="$1"
  if [[ "${value}" == \"*\" && "${value}" == *\" ]]; then
    printf '%s' "${value:1:${#value}-2}"
    return
  fi
  if [[ "${value}" == \'*\' && "${value}" == *\' ]]; then
    printf '%s' "${value:1:${#value}-2}"
    return
  fi
  printf '%s' "${value}"
}

load_env_var_from_file() {
  local key="$1"
  local file="$2"
  local line stripped value
  [[ -f "${file}" ]] || return 1

  while IFS= read -r line || [[ -n "${line}" ]]; do
    stripped="$(trim_whitespace "${line%%#*}")"
    [[ -z "${stripped}" ]] && continue
    if [[ "${stripped}" =~ ^export[[:space:]]+ ]]; then
      stripped="$(trim_whitespace "${stripped#export }")"
    fi
    if [[ "${stripped}" =~ ^${key}[[:space:]]*=[[:space:]]*(.*)$ ]]; then
      value="$(trim_whitespace "${BASH_REMATCH[1]}")"
      strip_wrapping_quotes "${value}"
      return 0
    fi
  done < "${file}"
  return 1
}

# Load local .env defaults without executing the file as shell code.
for env_file in "${ROOT_DIR}/.env.local" "${ROOT_DIR}/.env"; do
  if [[ -z "${REMOTE_TEST_HOST:-}" ]]; then
    if value="$(load_env_var_from_file "REMOTE_TEST_HOST" "${env_file}")"; then
      REMOTE_TEST_HOST="${value}"
    fi
  fi
  if [[ -z "${REMOTE_TEST_REMOTE_DIR:-}" ]]; then
    if value="$(load_env_var_from_file "REMOTE_TEST_REMOTE_DIR" "${env_file}")"; then
      REMOTE_TEST_REMOTE_DIR="${value}"
    fi
  fi
done

HOST="${HOST:-${REMOTE_TEST_HOST:-}}"
REMOTE_DIR="${REMOTE_TEST_REMOTE_DIR:-${REMOTE_DIR}}"

usage() {
  cat <<USAGE
Usage: scripts/remote-test.sh [options]

Sync this repository to a remote host over SSH and run Rust tests there.
If REMOTE_TEST_HOST is not set in your shell, the script will read .env.local
or .env from the repository root (keys only, no shell execution).

Options:
  --host <host>            SSH host (default: \$REMOTE_TEST_HOST)
  --remote-dir <path>      Remote project dir (default: \$REMOTE_TEST_REMOTE_DIR or ${REMOTE_DIR})
  --mode <native|docker>   Test mode (default: ${MODE})
  --docker                 Shortcut for --mode docker
  --native                 Shortcut for --mode native
  --no-sync                Skip file sync step
  --cargo-args '<args>'    Extra args appended to cargo test
  -h, --help               Show this help

Examples:
  REMOTE_TEST_HOST=your-host scripts/remote-test.sh
  REMOTE_TEST_REMOTE_DIR=~/work/kelvinclaw scripts/remote-test.sh --native
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
  cat >&2 <<MSG
Missing remote host. Pass --host <host> or set REMOTE_TEST_HOST.
Tip:
  cp .env.example .env
  # then set REMOTE_TEST_HOST in .env
MSG
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
