#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="${ROOT_DIR}/dist/releases"
TARGET=""
VERSION=""
TARGET_DIR="${ROOT_DIR}/target/releases"
EMIT_DEB="false"
SKIP_SMOKE_TEST="false"

# shellcheck source=scripts/lib/rust-toolchain-path.sh
source "${ROOT_DIR}/scripts/lib/rust-toolchain-path.sh"

usage() {
  cat <<'USAGE'
Usage: scripts/package-unix-release.sh --target <target-triple> [options]

Build KelvinClaw release executables for a Linux or macOS target, package them
into a tarball, optionally emit a Debian package for Linux targets, and
smoke-test the produced artifacts.

Options:
  --target <triple>        Required Rust target triple
  --output-dir <path>      Directory for release artifacts (default: ./dist/releases)
  --target-dir <path>      Cargo target dir (default: ./target/releases)
  --version <semver>       Override version label (default: workspace version)
  --emit-deb               Also build a .deb package for Linux targets
  --skip-smoke-test        Skip archive binary smoke tests
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

build_release_binaries() {
  local target="$1"
  local target_dir="$2"
  CARGO_TARGET_DIR="${target_dir}" cargo build \
    --locked \
    --release \
    --target "${target}" \
    -p kelvin-host \
    -p kelvin-gateway \
    -p kelvin-registry \
    -p kelvin-memory-controller \
    --features kelvin-gateway/memory_rpc
}

target_architecture() {
  case "$1" in
    x86_64-unknown-linux-gnu|x86_64-apple-darwin) printf '%s\n' 'x86_64' ;;
    aarch64-unknown-linux-gnu|aarch64-apple-darwin) printf '%s\n' 'aarch64' ;;
    *) echo "Unsupported target triple: $1" >&2; exit 1 ;;
  esac
}

host_architecture() {
  case "$(uname -m)" in
    x86_64|amd64) printf '%s\n' 'x86_64' ;;
    arm64|aarch64) printf '%s\n' 'aarch64' ;;
    *) uname -m ;;
  esac
}

sha256_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${file}" | awk '{print $1}'
    return 0
  fi
  shasum -a 256 "${file}" | awk '{print $1}'
}

workspace_version() {
  cargo metadata --no-deps --format-version 1 \
    | jq -r '.packages[] | select(.name == "kelvin-host") | .version'
}

platform_label() {
  case "$1" in
    x86_64-unknown-linux-gnu) printf '%s\n' 'linux-x86_64' ;;
    aarch64-unknown-linux-gnu) printf '%s\n' 'linux-arm64' ;;
    x86_64-apple-darwin) printf '%s\n' 'macos-x86_64' ;;
    aarch64-apple-darwin) printf '%s\n' 'macos-arm64' ;;
    *) echo "Unsupported target triple: $1" >&2; exit 1 ;;
  esac
}

deb_architecture() {
  case "$1" in
    x86_64-unknown-linux-gnu) printf '%s\n' 'amd64' ;;
    aarch64-unknown-linux-gnu) printf '%s\n' 'arm64' ;;
    *) echo "Debian packages are only supported for Linux targets" >&2; exit 1 ;;
  esac
}

create_tar_gz() {
  local output_path="$1"
  local base_dir="$2"
  local root_name="$3"
  local -a tar_args=(-czf "${output_path}" -C "${base_dir}" "${root_name}")

  if tar --help 2>/dev/null | grep -q -- '--sort='; then
    tar_args=(--sort=name --mtime='UTC 1970-01-01' --owner=0 --group=0 --numeric-owner "${tar_args[@]}")
  fi
  if tar --help 2>/dev/null | grep -q -- '--format'; then
    tar_args=(--format ustar "${tar_args[@]}")
  fi
  if tar --help 2>/dev/null | grep -q -- '--no-xattrs'; then
    tar_args=(--no-xattrs "${tar_args[@]}")
  fi
  if tar --help 2>/dev/null | grep -q -- '--no-acls'; then
    tar_args=(--no-acls "${tar_args[@]}")
  fi
  if tar --help 2>/dev/null | grep -q -- '--no-selinux'; then
    tar_args=(--no-selinux "${tar_args[@]}")
  fi

  COPYFILE_DISABLE=1 COPY_EXTENDED_ATTRIBUTES_DISABLE=1 tar "${tar_args[@]}"
}

smoke_test_archive() {
  local archive_path="$1"
  local root_name="$2"
  local bin_suffix="$3"
  local launcher_path="$4"
  local work_dir=""

  work_dir="$(mktemp -d)"
  tar -xzf "${archive_path}" -C "${work_dir}"
  "${work_dir}/${root_name}/${launcher_path}" --help >/dev/null
  "${work_dir}/${root_name}/bin/kelvin-host${bin_suffix}" --help >/dev/null
  "${work_dir}/${root_name}/bin/kelvin-gateway${bin_suffix}" --help >/dev/null
  "${work_dir}/${root_name}/bin/kelvin-registry${bin_suffix}" --help >/dev/null
  "${work_dir}/${root_name}/bin/kelvin-memory-controller${bin_suffix}" --help >/dev/null
  rm -rf "${work_dir}"
}

create_deb_package() {
  local stage_root="$1"
  local version="$2"
  local target="$3"
  local output_dir="$4"
  local work_dir=""
  local package_root=""
  local install_root=""
  local deb_arch=""
  local deb_path=""
  local checksum_path=""
  local extract_root=""

  require_cmd dpkg-deb

  deb_arch="$(deb_architecture "${target}")"
  deb_path="${output_dir}/kelvinclaw_${version}_${deb_arch}.deb"
  checksum_path="${deb_path}.sha256"
  work_dir="$(mktemp -d)"
  package_root="${work_dir}/pkg"
  install_root="${package_root}/usr/lib/kelvinclaw"
  extract_root="${work_dir}/extract"

  mkdir -p "${install_root}" "${package_root}/usr/bin" "${package_root}/DEBIAN"
  cp -R "${stage_root}/." "${install_root}/"
  ln -s ../lib/kelvinclaw/kelvin "${package_root}/usr/bin/kelvin"

  cat > "${package_root}/DEBIAN/control" <<EOF
Package: kelvinclaw
Version: ${version}
Section: utils
Priority: optional
Architecture: ${deb_arch}
Maintainer: AgenticHighway
Depends: ca-certificates, curl, tar
Description: KelvinClaw release bundle
 KelvinClaw runtime bundle with first-party plugin bootstrap support.
EOF

  rm -f "${deb_path}" "${checksum_path}"
  dpkg-deb --build --root-owner-group "${package_root}" "${deb_path}" >/dev/null
  printf '%s  %s\n' "$(sha256_file "${deb_path}")" "$(basename "${deb_path}")" > "${checksum_path}"

  mkdir -p "${extract_root}"
  dpkg-deb -x "${deb_path}" "${extract_root}"
  "${extract_root}/usr/lib/kelvinclaw/kelvin" --help >/dev/null
  "${extract_root}/usr/lib/kelvinclaw/bin/kelvin-host" --help >/dev/null
  "${extract_root}/usr/lib/kelvinclaw/bin/kelvin-gateway" --help >/dev/null
  "${extract_root}/usr/lib/kelvinclaw/bin/kelvin-registry" --help >/dev/null
  "${extract_root}/usr/lib/kelvinclaw/bin/kelvin-memory-controller" --help >/dev/null

  rm -rf "${work_dir}"
  echo "deb=${deb_path}"
  echo "deb_checksum=${checksum_path}"
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
    --emit-deb)
      EMIT_DEB="true"
      shift
      ;;
    --skip-smoke-test)
      SKIP_SMOKE_TEST="true"
      shift
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

ensure_rust_toolchain_path || true
require_cmd jq
require_cmd tar
require_cmd cargo
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

mkdir -p "${OUTPUT_DIR}" "${STAGE_ROOT}/bin" "${STAGE_ROOT}/share"

rustup target add "${TARGET}" >/dev/null

build_release_binaries "${TARGET}" "${TARGET_DIR}"

cp "${TARGET_DIR}/${TARGET}/release/kelvin-host" "${STAGE_ROOT}/bin/"
cp "${TARGET_DIR}/${TARGET}/release/kelvin-gateway" "${STAGE_ROOT}/bin/"
cp "${TARGET_DIR}/${TARGET}/release/kelvin-registry" "${STAGE_ROOT}/bin/"
cp "${TARGET_DIR}/${TARGET}/release/kelvin-memory-controller" "${STAGE_ROOT}/bin/"
cp "${ROOT_DIR}/LICENSE" "${STAGE_ROOT}/"
cp "${ROOT_DIR}/README.md" "${STAGE_ROOT}/"
cp "${ROOT_DIR}/scripts/kelvin-release-launcher.sh" "${STAGE_ROOT}/kelvin"
cp "${ROOT_DIR}/release/official-first-party-plugins.env" "${STAGE_ROOT}/share/official-first-party-plugins.env"
chmod +x "${STAGE_ROOT}/kelvin"

if command -v xattr >/dev/null 2>&1; then
  xattr -rc "${STAGE_ROOT}" >/dev/null 2>&1 || true
fi

cat > "${STAGE_ROOT}/BUILD_INFO.txt" <<EOF
version=${VERSION}
target=${TARGET}
platform=${PLATFORM_LABEL}
required_plugin=kelvin.cli@$(awk -F'"' '/^KELVIN_CLI_VERSION=/ {print $2}' "${ROOT_DIR}/release/official-first-party-plugins.env")
optional_plugin=kelvin.openai@$(awk -F'"' '/^KELVIN_OPENAI_VERSION=/ {print $2}' "${ROOT_DIR}/release/official-first-party-plugins.env")
EOF

rm -f "${ARCHIVE_PATH}" "${CHECKSUM_PATH}"
create_tar_gz "${ARCHIVE_PATH}" "${STAGE_PARENT}" "${ARCHIVE_ROOT}"
printf '%s  %s\n' "$(sha256_file "${ARCHIVE_PATH}")" "$(basename "${ARCHIVE_PATH}")" > "${CHECKSUM_PATH}"
if [[ "${SKIP_SMOKE_TEST}" == "true" ]]; then
  echo "Skipping archive smoke test (--skip-smoke-test)"
elif [[ "$(target_architecture "${TARGET}")" != "$(host_architecture)" ]]; then
  echo "Skipping archive smoke test for cross-arch target ${TARGET} on host $(host_architecture)"
else
  smoke_test_archive "${ARCHIVE_PATH}" "${ARCHIVE_ROOT}" "" "kelvin"
fi

if [[ "${EMIT_DEB}" == "true" ]]; then
  create_deb_package "${STAGE_ROOT}" "${VERSION}" "${TARGET}" "${OUTPUT_DIR}"
fi

echo "archive=${ARCHIVE_PATH}"
echo "checksum=${CHECKSUM_PATH}"
