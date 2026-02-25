#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN_HOME_DEFAULT="${HOME}/.kelvinclaw/plugins"
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${PLUGIN_HOME_DEFAULT}}"

PACKAGE_PATH=""
FORCE="0"

usage() {
  cat <<'USAGE'
Usage: scripts/plugin-install.sh --package <plugin-package.tar.gz> [options]

Install a prebuilt Kelvin SDK plugin package without compiling Rust code.

Required package layout:
  plugin.json
  payload/<files...>

Required plugin.json fields:
  id, name, version, api_version, entrypoint, capabilities

Options:
  --package <path>    Path to plugin package (.tar.gz)
  --force             Overwrite existing plugin version
  --plugin-home <dir> Install root (default: $KELVIN_PLUGIN_HOME or ~/.kelvinclaw/plugins)
  -h, --help          Show this help

Examples:
  scripts/plugin-install.sh --package ./dist/acme.echo-1.0.0.tar.gz
  KELVIN_PLUGIN_HOME=./.kelvin/plugins scripts/plugin-install.sh --package ./dist/acme.echo-1.0.0.tar.gz --force
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --package)
      PACKAGE_PATH="${2:?missing value for --package}"
      shift 2
      ;;
    --plugin-home)
      PLUGIN_HOME="${2:?missing value for --plugin-home}"
      shift 2
      ;;
    --force)
      FORCE="1"
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

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 1
  fi
}

require_cmd tar
require_cmd jq
require_cmd shasum

if [[ -z "${PACKAGE_PATH}" ]]; then
  echo "Missing --package <path>." >&2
  usage
  exit 1
fi

if [[ ! -f "${PACKAGE_PATH}" ]]; then
  echo "Package not found: ${PACKAGE_PATH}" >&2
  exit 1
fi

WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

tar -xzf "${PACKAGE_PATH}" -C "${WORK_DIR}"

MANIFEST_PATH="${WORK_DIR}/plugin.json"
PAYLOAD_DIR="${WORK_DIR}/payload"

if [[ ! -f "${MANIFEST_PATH}" ]]; then
  echo "Invalid package: missing plugin.json" >&2
  exit 1
fi
if [[ ! -d "${PAYLOAD_DIR}" ]]; then
  echo "Invalid package: missing payload/ directory" >&2
  exit 1
fi

manifest_get() {
  local expr="$1"
  jq -er "${expr}" "${MANIFEST_PATH}"
}

PLUGIN_ID="$(manifest_get '.id')"
PLUGIN_NAME="$(manifest_get '.name')"
PLUGIN_VERSION="$(manifest_get '.version')"
API_VERSION="$(manifest_get '.api_version')"
ENTRYPOINT_REL="$(manifest_get '.entrypoint')"
CAPS_COUNT="$(manifest_get '.capabilities | length')"

if [[ "${CAPS_COUNT}" -lt 1 ]]; then
  echo "Invalid plugin.json: capabilities must contain at least one entry" >&2
  exit 1
fi

if [[ "${ENTRYPOINT_REL}" == /* || "${ENTRYPOINT_REL}" == *".."* ]]; then
  echo "Invalid plugin.json: entrypoint must be a safe relative path" >&2
  exit 1
fi

ENTRYPOINT_ABS="${PAYLOAD_DIR}/${ENTRYPOINT_REL}"
if [[ ! -f "${ENTRYPOINT_ABS}" ]]; then
  echo "Invalid package: entrypoint file not found: payload/${ENTRYPOINT_REL}" >&2
  exit 1
fi

EXPECTED_SHA="$(jq -er '.entrypoint_sha256 // empty' "${MANIFEST_PATH}")"
if [[ -n "${EXPECTED_SHA}" ]]; then
  ACTUAL_SHA="$(shasum -a 256 "${ENTRYPOINT_ABS}" | awk '{print $1}')"
  if [[ "${ACTUAL_SHA}" != "${EXPECTED_SHA}" ]]; then
    echo "Checksum mismatch for entrypoint. expected=${EXPECTED_SHA} actual=${ACTUAL_SHA}" >&2
    exit 1
  fi
fi

INSTALL_DIR="${PLUGIN_HOME}/${PLUGIN_ID}/${PLUGIN_VERSION}"
CURRENT_LINK="${PLUGIN_HOME}/${PLUGIN_ID}/current"

if [[ -e "${INSTALL_DIR}" && "${FORCE}" != "1" ]]; then
  echo "Plugin already installed at ${INSTALL_DIR}. Use --force to overwrite." >&2
  exit 1
fi

mkdir -p "${PLUGIN_HOME}/${PLUGIN_ID}"
rm -rf "${INSTALL_DIR}"
mkdir -p "${INSTALL_DIR}"
cp "${MANIFEST_PATH}" "${INSTALL_DIR}/plugin.json"
cp -R "${PAYLOAD_DIR}" "${INSTALL_DIR}/payload"
ln -sfn "${PLUGIN_VERSION}" "${CURRENT_LINK}"

echo "Installed plugin:"
echo "  id:          ${PLUGIN_ID}"
echo "  name:        ${PLUGIN_NAME}"
echo "  version:     ${PLUGIN_VERSION}"
echo "  api_version: ${API_VERSION}"
echo "  path:        ${INSTALL_DIR}"
echo "  current:     ${CURRENT_LINK} -> ${PLUGIN_VERSION}"

