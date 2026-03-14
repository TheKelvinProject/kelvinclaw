#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALLER="${ROOT_DIR}/scripts/plugin-install.sh"
INDEX_INSTALLER="${ROOT_DIR}/scripts/plugin-index-install.sh"
LISTER="${ROOT_DIR}/scripts/plugin-list.sh"
UNINSTALLER="${ROOT_DIR}/scripts/plugin-uninstall.sh"
DISCOVERY="${ROOT_DIR}/scripts/plugin-discovery.sh"
UPDATE_CHECK="${ROOT_DIR}/scripts/plugin-update-check.sh"

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 1
  fi
}

require_cmd jq
require_cmd tar

sha256_file() {
  local file="$1"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${file}" | awk '{print $1}'
    return
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${file}" | awk '{print $1}'
    return
  fi
  echo "Missing required command: shasum or sha256sum" >&2
  exit 1
}

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

SHA="$(sha256_file "${PAYLOAD_DIR}/echo.wasm")"
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
SHA_V2="$(sha256_file "${PACKAGE_V2_DIR}/payload/echo.wasm")"
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

# Remote index install should prefer the highest semver-compatible version, not lexical order.
INDEX_PLUGIN_HOME="${WORK_DIR}/index-plugin-home"
INDEX_TRUST_POLICY="${WORK_DIR}/index-trusted-publishers.json"
INDEX_PACKAGE_ROOT="${WORK_DIR}/index-packages"
mkdir -p "${INDEX_PACKAGE_ROOT}"

build_index_package() {
  local version="$1"
  local package_dir="${WORK_DIR}/package-index-${version}"
  mkdir -p "${package_dir}/payload"
  printf 'index-plugin-%s\n' "${version}" > "${package_dir}/payload/echo.wasm"
  local sha
  sha="$(sha256_file "${package_dir}/payload/echo.wasm")"
  cat > "${package_dir}/plugin.json" <<JSON
{
  "id": "acme.indexed",
  "name": "Acme Indexed",
  "version": "${version}",
  "api_version": "1.0.0",
  "entrypoint": "echo.wasm",
  "entrypoint_sha256": "${sha}",
  "capabilities": ["tool_provider"],
  "quality_tier": "signed_trusted"
}
JSON
  local tarball="${INDEX_PACKAGE_ROOT}/acme.indexed-${version}.tar.gz"
  tar -czf "${tarball}" -C "${package_dir}" .
  printf '%s' "${tarball}"
}

PACKAGE_020="$(build_index_package "0.2.0")"
PACKAGE_0100="$(build_index_package "0.10.0")"
SHA_020="$(sha256_file "${PACKAGE_020}")"
SHA_0100="$(sha256_file "${PACKAGE_0100}")"
TRUST_POLICY_SOURCE="${WORK_DIR}/trusted_publishers.json"
cat > "${TRUST_POLICY_SOURCE}" <<'JSON'
{
  "require_signature": true,
  "publishers": [{"id": "acme", "ed25519_public_key": "fixture"}]
}
JSON
INDEX_JSON="${WORK_DIR}/plugin-index.json"
cat > "${INDEX_JSON}" <<JSON
{
  "schema_version": "v1",
  "plugins": [
    {
      "id": "acme.indexed",
      "version": "0.2.0",
      "package_url": "file://${PACKAGE_020}",
      "sha256": "${SHA_020}",
      "trust_policy_url": "file://${TRUST_POLICY_SOURCE}",
      "quality_tier": "signed_trusted",
      "tags": ["example"]
    },
    {
      "id": "acme.indexed",
      "version": "0.10.0",
      "package_url": "file://${PACKAGE_0100}",
      "sha256": "${SHA_0100}",
      "trust_policy_url": "file://${TRUST_POLICY_SOURCE}",
      "quality_tier": "signed_trusted",
      "tags": ["example", "latest"]
    }
  ]
}
JSON

"${INDEX_INSTALLER}" \
  --plugin acme.indexed \
  --index-url "file://${INDEX_JSON}" \
  --plugin-home "${INDEX_PLUGIN_HOME}" \
  --trust-policy-path "${INDEX_TRUST_POLICY}" >/dev/null

[[ "$(readlink "${INDEX_PLUGIN_HOME}/acme.indexed/current")" == "0.10.0" ]]

DISCOVERY_JSON="$("${DISCOVERY}" --index-url "file://${INDEX_JSON}" --plugin acme.indexed --json)"
echo "${DISCOVERY_JSON}" | jq -e '.[0].version == "0.10.0" and .[1].version == "0.2.0"' >/dev/null

UPDATE_JSON="$("${UPDATE_CHECK}" \
  --plugin-home "${INDEX_PLUGIN_HOME}" \
  --index-url "file://${INDEX_JSON}" \
  --plugin acme.indexed \
  --json)"
echo "${UPDATE_JSON}" | jq -e '
  length == 1 and
  .[0].installed_version == "0.10.0" and
  .[0].latest_version == "0.10.0" and
  .[0].update_available == false
' >/dev/null

KELVIN_PLUGIN_HOME="${INDEX_PLUGIN_HOME}" "${UNINSTALLER}" --id acme.indexed --version 0.10.0 >/dev/null
[[ "$(readlink "${INDEX_PLUGIN_HOME}/acme.indexed/current")" == "0.2.0" ]]

echo "[test-plugin-install] success"
