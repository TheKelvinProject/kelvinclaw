#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="${ROOT_DIR}/dist/releases"
TARGET=""
VERSION=""
TARGET_DIR="${ROOT_DIR}/target/releases"

usage() {
  cat <<'USAGE'
Usage: scripts/package-linux-release.sh --target <target-triple> [options]

Build KelvinClaw release executables for a Linux target, package them into a
deterministic tarball, and smoke-test the packaged binaries.

Options:
  --target <triple>        Required Rust target triple
  --output-dir <path>      Directory for release tarballs (default: ./dist/releases)
  --target-dir <path>      Cargo target dir (default: ./target/releases)
  --version <semver>       Override version label (default: workspace version)
  -h, --help               Show help
USAGE
}

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 1
  fi
}

sha256_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${file}" | awk '{print $1}'
    return 0
  fi
  shasum -a 256 "${file}" | awk '{print $1}'
}

platform_label() {
  case "$1" in
    x86_64-unknown-linux-gnu) printf '%s\n' 'linux-x86_64' ;;
    aarch64-unknown-linux-gnu) printf '%s\n' 'linux-arm64' ;;
    *) echo "Unsupported target triple: $1" >&2; exit 1 ;;
  esac
}

workspace_version() {
  cargo metadata --no-deps --format-version 1 \
    | jq -r '.packages[] | select(.name == "kelvin-host") | .version'
}

create_tar_gz() {
  local output_path="$1"
  local base_dir="$2"
  local root_name="$3"

  tar \
    --sort=name \
    --mtime='UTC 1970-01-01' \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -czf "${output_path}" \
    -C "${base_dir}" \
    "${root_name}"
}

smoke_test_archive() {
  local archive_path="$1"
  local root_name="$2"
  local work_dir=""

  work_dir="$(mktemp -d)"
  tar -xzf "${archive_path}" -C "${work_dir}"
  "${work_dir}/${root_name}/bin/kelvin-host" --help >/dev/null
  "${work_dir}/${root_name}/bin/kelvin-gateway" --help >/dev/null
  "${work_dir}/${root_name}/bin/kelvin-registry" --help >/dev/null
  "${work_dir}/${root_name}/bin/kelvin-memory-controller" --help >/dev/null
  rm -rf "${work_dir}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      TARGET="${2:?missing value for --target}"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="${2:?missing value for --output-dir}"
      shift 2
      ;;
    --target-dir)
      TARGET_DIR="${2:?missing value for --target-dir}"
      shift 2
      ;;
    --version)
      VERSION="${2:?missing value for --version}"
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

[[ -n "${TARGET}" ]] || {
  echo "--target is required" >&2
  usage
  exit 1
}

require_cmd cargo
require_cmd jq
require_cmd tar
require_cmd rustup

if [[ -z "${VERSION}" ]]; then
  VERSION="$(workspace_version)"
fi

PLATFORM_LABEL="$(platform_label "${TARGET}")"
ARCHIVE_ROOT="kelvinclaw-${VERSION}-${PLATFORM_LABEL}"
ARCHIVE_PATH="${OUTPUT_DIR}/${ARCHIVE_ROOT}.tar.gz"
CHECKSUM_PATH="${ARCHIVE_PATH}.sha256"
STAGE_PARENT="$(mktemp -d)"
STAGE_ROOT="${STAGE_PARENT}/${ARCHIVE_ROOT}"

cleanup() {
  rm -rf "${STAGE_PARENT}"
}
trap cleanup EXIT

mkdir -p "${OUTPUT_DIR}" "${STAGE_ROOT}/bin"

rustup target add "${TARGET}" >/dev/null

CARGO_TARGET_DIR="${TARGET_DIR}" cargo build --locked --release --target "${TARGET}" -p kelvin-host
CARGO_TARGET_DIR="${TARGET_DIR}" cargo build --locked --release --target "${TARGET}" -p kelvin-gateway --features memory_rpc
CARGO_TARGET_DIR="${TARGET_DIR}" cargo build --locked --release --target "${TARGET}" -p kelvin-registry
CARGO_TARGET_DIR="${TARGET_DIR}" cargo build --locked --release --target "${TARGET}" -p kelvin-memory-controller

cp "${TARGET_DIR}/${TARGET}/release/kelvin-host" "${STAGE_ROOT}/bin/"
cp "${TARGET_DIR}/${TARGET}/release/kelvin-gateway" "${STAGE_ROOT}/bin/"
cp "${TARGET_DIR}/${TARGET}/release/kelvin-registry" "${STAGE_ROOT}/bin/"
cp "${TARGET_DIR}/${TARGET}/release/kelvin-memory-controller" "${STAGE_ROOT}/bin/"
cp "${ROOT_DIR}/LICENSE" "${STAGE_ROOT}/"
cp "${ROOT_DIR}/README.md" "${STAGE_ROOT}/"

cat > "${STAGE_ROOT}/BUILD_INFO.txt" <<EOF
version=${VERSION}
target=${TARGET}
platform=${PLATFORM_LABEL}
EOF

rm -f "${ARCHIVE_PATH}" "${CHECKSUM_PATH}"
create_tar_gz "${ARCHIVE_PATH}" "${STAGE_PARENT}" "${ARCHIVE_ROOT}"
printf '%s  %s\n' "$(sha256_file "${ARCHIVE_PATH}")" "$(basename "${ARCHIVE_PATH}")" > "${CHECKSUM_PATH}"
smoke_test_archive "${ARCHIVE_PATH}" "${ARCHIVE_ROOT}"

echo "archive=${ARCHIVE_PATH}"
echo "checksum=${CHECKSUM_PATH}"
