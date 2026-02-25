#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALLER="${ROOT_DIR}/scripts/plugin-install.sh"
LISTER="${ROOT_DIR}/scripts/plugin-list.sh"
UNINSTALLER="${ROOT_DIR}/scripts/plugin-uninstall.sh"

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 1
  fi
}

require_cmd jq
require_cmd tar
require_cmd shasum

WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

PLUGIN_HOME="${WORK_DIR}/plugin-home"
PACKAGE_DIR="${WORK_DIR}/package"
PAYLOAD_DIR="${PACKAGE_DIR}/payload"
DIST_DIR="${WORK_DIR}/dist"
mkdir -p "${PAYLOAD_DIR}" "${DIST_DIR}"

cat > "${PAYLOAD_DIR}/echo.wasm" <<'WASM'
fake-wasm-bytes
WASM

SHA="$(shasum -a 256 "${PAYLOAD_DIR}/echo.wasm" | awk '{print $1}')"
cat > "${PACKAGE_DIR}/plugin.json" <<JSON
{
  "id": "acme.echo",
  "name": "Acme Echo",
  "version": "1.0.0",
  "api_version": "1.0.0",
  "entrypoint": "echo.wasm",
  "entrypoint_sha256": "${SHA}",
  "capabilities": ["tool_provider"]
}
JSON

PACKAGE_TARBALL="${DIST_DIR}/acme.echo-1.0.0.tar.gz"
tar -czf "${PACKAGE_TARBALL}" -C "${PACKAGE_DIR}" .

KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" "${INSTALLER}" --package "${PACKAGE_TARBALL}" >/dev/null

INSTALLED_MANIFEST="${PLUGIN_HOME}/acme.echo/1.0.0/plugin.json"
INSTALLED_ENTRYPOINT="${PLUGIN_HOME}/acme.echo/1.0.0/payload/echo.wasm"
CURRENT_LINK="${PLUGIN_HOME}/acme.echo/current"

test -f "${INSTALLED_MANIFEST}"
test -f "${INSTALLED_ENTRYPOINT}"
test -L "${CURRENT_LINK}"
[[ "$(readlink "${CURRENT_LINK}")" == "1.0.0" ]]

LIST_JSON="$(KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" "${LISTER}" --json)"
echo "${LIST_JSON}" | jq -e '
  length == 1 and
  .[0].id == "acme.echo" and
  .[0].version == "1.0.0" and
  .[0].is_current == true
' >/dev/null

# Reject duplicate install without --force.
if KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" "${INSTALLER}" --package "${PACKAGE_TARBALL}" >/dev/null 2>&1; then
  echo "Expected duplicate install to fail without --force" >&2
  exit 1
fi

# Reject bad checksum.
BAD_PACKAGE_DIR="${WORK_DIR}/bad-package"
mkdir -p "${BAD_PACKAGE_DIR}/payload"
cp "${PAYLOAD_DIR}/echo.wasm" "${BAD_PACKAGE_DIR}/payload/echo.wasm"
cat > "${BAD_PACKAGE_DIR}/plugin.json" <<'JSON'
{
  "id": "acme.bad",
  "name": "Acme Bad",
  "version": "1.0.0",
  "api_version": "1.0.0",
  "entrypoint": "echo.wasm",
  "entrypoint_sha256": "deadbeef",
  "capabilities": ["tool_provider"]
}
JSON
BAD_TARBALL="${DIST_DIR}/acme.bad-1.0.0.tar.gz"
tar -czf "${BAD_TARBALL}" -C "${BAD_PACKAGE_DIR}" .
if KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" "${INSTALLER}" --package "${BAD_TARBALL}" >/dev/null 2>&1; then
  echo "Expected checksum validation failure" >&2
  exit 1
fi

# Install second version and verify current pointer.
PACKAGE_V2_DIR="${WORK_DIR}/package-v2"
mkdir -p "${PACKAGE_V2_DIR}/payload"
cat > "${PACKAGE_V2_DIR}/payload/echo.wasm" <<'WASM'
fake-wasm-bytes-v2
WASM
SHA_V2="$(shasum -a 256 "${PACKAGE_V2_DIR}/payload/echo.wasm" | awk '{print $1}')"
cat > "${PACKAGE_V2_DIR}/plugin.json" <<JSON
{
  "id": "acme.echo",
  "name": "Acme Echo",
  "version": "2.0.0",
  "api_version": "1.0.0",
  "entrypoint": "echo.wasm",
  "entrypoint_sha256": "${SHA_V2}",
  "capabilities": ["tool_provider"]
}
JSON
PACKAGE_V2_TARBALL="${DIST_DIR}/acme.echo-2.0.0.tar.gz"
tar -czf "${PACKAGE_V2_TARBALL}" -C "${PACKAGE_V2_DIR}" .
KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" "${INSTALLER}" --package "${PACKAGE_V2_TARBALL}" >/dev/null
[[ "$(readlink "${CURRENT_LINK}")" == "2.0.0" ]]

# Uninstall current version: should switch current to previous version.
KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" "${UNINSTALLER}" --id acme.echo --version 2.0.0 >/dev/null
[[ "$(readlink "${CURRENT_LINK}")" == "1.0.0" ]]

# Purge should remove plugin directory.
KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" "${UNINSTALLER}" --id acme.echo --purge >/dev/null
if [[ -d "${PLUGIN_HOME}/acme.echo" ]]; then
  echo "Expected plugin directory to be removed by purge" >&2
  exit 1
fi

LIST_EMPTY="$(KELVIN_PLUGIN_HOME="${PLUGIN_HOME}" "${LISTER}" --json)"
echo "${LIST_EMPTY}" | jq -e 'length == 0' >/dev/null

echo "[test-plugin-install] success"
