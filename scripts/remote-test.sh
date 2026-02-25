#!/usr/bin/env bash
set -euo pipefail

HOST="${REMOTE_TEST_HOST:-}"
REMOTE_DIR="${REMOTE_TEST_REMOTE_DIR:-~/kelvinclaw}"
DOCKER_IMAGE="${REMOTE_TEST_DOCKER_IMAGE:-rust:1.93.1-bookworm}"
MODE="native"
DO_SYNC="1"
EXTRA_CARGO_ARGS=""
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SYNC_EXCLUDES=(
  ".git"
  "target"
  ".DS_Store"
  ".bench"
  ".cache"
  ".kelvin"
  ".env"
  ".env.local"
)

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 1
  fi
}

is_safe_remote_host() {
  local value="$1"
  [[ "${value}" =~ ^[A-Za-z0-9._@:-]+$ ]]
}

is_safe_remote_dir_for_rsync() {
  local value="$1"
  [[ "${value}" =~ ^[A-Za-z0-9._/~+-]+$ ]]
}

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
  if [[ -z "${REMOTE_TEST_DOCKER_IMAGE:-}" ]]; then
    if value="$(load_env_var_from_file "REMOTE_TEST_DOCKER_IMAGE" "${env_file}")"; then
      REMOTE_TEST_DOCKER_IMAGE="${value}"
    fi
  fi
done

HOST="${HOST:-${REMOTE_TEST_HOST:-}}"
REMOTE_DIR="${REMOTE_TEST_REMOTE_DIR:-${REMOTE_DIR}}"
DOCKER_IMAGE="${REMOTE_TEST_DOCKER_IMAGE:-${DOCKER_IMAGE}}"

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
  --docker-image <image>   Docker image for docker mode (default: ${DOCKER_IMAGE})
  --no-sync                Skip file sync step
  --cargo-args '<args>'    Extra args appended to cargo test
  -h, --help               Show this help

Examples:
  REMOTE_TEST_HOST=your-host scripts/remote-test.sh
  REMOTE_TEST_REMOTE_DIR=~/work/kelvinclaw scripts/remote-test.sh --native
  REMOTE_TEST_DOCKER_IMAGE=rust:1.93.1-bookworm scripts/remote-test.sh --docker
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
    --docker-image)
      DOCKER_IMAGE="${2:?missing value for --docker-image}"
      shift 2
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

require_cmd ssh

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
if [[ "${MODE}" == "docker" ]]; then
  echo "[remote-test] docker_image=${DOCKER_IMAGE}"
fi

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
  if command -v rsync >/dev/null 2>&1 \
    && is_safe_remote_host "${HOST}" \
    && is_safe_remote_dir_for_rsync "${REMOTE_DIR}"; then
    rsync_args=(-az --delete)
    for pattern in "${SYNC_EXCLUDES[@]}"; do
      rsync_args+=(--exclude "${pattern}")
    done
    rsync "${rsync_args[@]}" "${ROOT_DIR}/" "${HOST}:${REMOTE_DIR}/"
  else
    tar_args=(czf -)
    for pattern in "${SYNC_EXCLUDES[@]}"; do
      tar_args+=(--exclude="${pattern}")
    done
    tar "${tar_args[@]}" -C "${ROOT_DIR}" . | ssh "${HOST}" bash -s -- "${REMOTE_DIR}" <<'EOF'
set -euo pipefail
remote_dir="$1"
remote_dir="${remote_dir/#\~/$HOME}"
rm -rf "${remote_dir}"
mkdir -p "${remote_dir}"
tar xzf - -C "${remote_dir}"
EOF
  fi
fi

if [[ "${MODE}" == "native" ]]; then
  echo "[remote-test] running native cargo test"
  ssh "${HOST}" bash -s -- "${REMOTE_DIR}" "${EXTRA_CARGO_ARGS}" <<'EOF'
set -euo pipefail
remote_dir="$1"
extra_cargo_args="$2"
remote_dir="${remote_dir/#\~/$HOME}"
source "$HOME/.cargo/env"
cd "${remote_dir}"
extra_args=()
if [[ -n "${extra_cargo_args}" ]]; then
  read -r -a extra_args <<< "${extra_cargo_args}"
fi
cargo test --workspace "${extra_args[@]}"
EOF
else
  echo "[remote-test] running cargo test in Docker"
  ssh "${HOST}" bash -s -- "${REMOTE_DIR}" "${DOCKER_IMAGE}" "${EXTRA_CARGO_ARGS}" <<'EOF'
set -euo pipefail
remote_dir="$1"
docker_image="$2"
extra_cargo_args="$3"
remote_dir="${remote_dir/#\~/$HOME}"
cd "${remote_dir}"
extra_args=()
if [[ -n "${extra_cargo_args}" ]]; then
  read -r -a extra_args <<< "${extra_cargo_args}"
fi
docker run --rm -v "${remote_dir}:/work" -w /work "${docker_image}" \
  cargo test --workspace "${extra_args[@]}"
EOF
fi

echo "[remote-test] done"
