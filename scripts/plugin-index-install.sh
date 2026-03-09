#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_INDEX_URL="https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json"
PLUGIN_HOME_DEFAULT="${HOME}/.kelvinclaw/plugins"
TRUST_POLICY_DEFAULT="${HOME}/.kelvinclaw/trusted_publishers.json"

INDEX_URL="${KELVIN_PLUGIN_INDEX_URL:-${DEFAULT_INDEX_URL}}"
PLUGIN_ID=""
PLUGIN_VERSION=""
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${PLUGIN_HOME_DEFAULT}}"
TRUST_POLICY_PATH="${KELVIN_TRUST_POLICY_PATH:-${TRUST_POLICY_DEFAULT}}"
FORCE="0"
MIN_QUALITY_TIER="${KELVIN_PLUGIN_MIN_QUALITY_TIER:-unsigned_local}"

usage() {
  cat <<'USAGE'
Usage: scripts/plugin-index-install.sh --plugin <id> [options]

Install a plugin package from a remote plugin index (no local Rust compilation).

Required options:
  --plugin <id>         Plugin id from index (example: kelvin.cli)

Optional:
  --index-url <url>     Plugin index JSON URL
                        (default: $KELVIN_PLUGIN_INDEX_URL or kelvinclaw-plugins main index)
  --version <version>   Version to install (defaults to highest semver for id)
  --plugin-home <dir>   Install root (default: $KELVIN_PLUGIN_HOME or ~/.kelvinclaw/plugins)
  --trust-policy-path <path>
                        Trust policy file (default: $KELVIN_TRUST_POLICY_PATH or ~/.kelvinclaw/trusted_publishers.json)
  --force               Reinstall even if version exists
  --min-quality-tier <tier>
                        Minimum accepted quality tier:
                        unsigned_local | signed_community | signed_trusted
                        (default: $KELVIN_PLUGIN_MIN_QUALITY_TIER or unsigned_local)
  -h, --help            Show help

Index schema (v1):
{
  "schema_version": "v1",
  "plugins": [
    {
      "id": "kelvin.cli",
      "version": "0.1.0",
      "package_url": "https://.../kelvin.cli-0.1.0.tar.gz",
      "sha256": "<required>",
      "trust_policy_url": "https://.../trusted_publishers.json"
    }
  ]
}
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --index-url)
      INDEX_URL="${2:?missing value for --index-url}"
      shift 2
      ;;
    --plugin)
      PLUGIN_ID="${2:?missing value for --plugin}"
      shift 2
      ;;
    --version)
      PLUGIN_VERSION="${2:?missing value for --version}"
      shift 2
      ;;
    --plugin-home)
      PLUGIN_HOME="${2:?missing value for --plugin-home}"
      shift 2
      ;;
    --trust-policy-path)
      TRUST_POLICY_PATH="${2:?missing value for --trust-policy-path}"
      shift 2
      ;;
    --force)
      FORCE="1"
      shift
      ;;
    --min-quality-tier)
      MIN_QUALITY_TIER="${2:?missing value for --min-quality-tier}"
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

tier_rank() {
  case "$1" in
    unsigned_local) echo 0 ;;
    signed_community) echo 1 ;;
    signed_trusted) echo 2 ;;
    *)
      echo "Invalid quality tier: $1" >&2
      exit 1
      ;;
  esac
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

require_cmd curl
require_cmd jq
require_cmd tar

if [[ -z "${PLUGIN_ID}" ]]; then
  echo "Missing --plugin <id>" >&2
  exit 1
fi

WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

INDEX_FILE="${WORK_DIR}/index.json"
echo "[plugin-index-install] fetching index: ${INDEX_URL}"
curl -fsSL "${INDEX_URL}" -o "${INDEX_FILE}"

if [[ "$(jq -r '.schema_version // "missing"' "${INDEX_FILE}")" != "v1" ]]; then
  echo "Invalid index: expected schema_version=v1" >&2
  exit 1
fi

SELECTOR='.plugins[] | select(.id == $id)'
if [[ -n "${PLUGIN_VERSION}" ]]; then
  SELECTOR="${SELECTOR} | select(.version == \$version)"
fi

PLUGIN_JSON="${WORK_DIR}/plugin-selection.json"
if [[ -n "${PLUGIN_VERSION}" ]]; then
  jq -c --arg id "${PLUGIN_ID}" --arg version "${PLUGIN_VERSION}" \
    "[${SELECTOR}] | .[0] // empty" "${INDEX_FILE}" > "${PLUGIN_JSON}"
else
  jq -c --arg id "${PLUGIN_ID}" \
    "[${SELECTOR}] | sort_by(.version) | reverse | .[0] // empty" "${INDEX_FILE}" > "${PLUGIN_JSON}"
fi

if [[ ! -s "${PLUGIN_JSON}" || "$(cat "${PLUGIN_JSON}")" == "null" ]]; then
  if [[ -n "${PLUGIN_VERSION}" ]]; then
    echo "Plugin not found in index: id=${PLUGIN_ID} version=${PLUGIN_VERSION}" >&2
  else
    echo "Plugin not found in index: id=${PLUGIN_ID}" >&2
  fi
  exit 1
fi

PACKAGE_URL="$(jq -r '.package_url // empty' "${PLUGIN_JSON}")"
EXPECTED_SHA="$(jq -r '.sha256 // empty' "${PLUGIN_JSON}")"
SELECTED_VERSION="$(jq -r '.version // empty' "${PLUGIN_JSON}")"
TRUST_POLICY_URL="$(jq -r '.trust_policy_url // empty' "${PLUGIN_JSON}")"
QUALITY_TIER="$(jq -r '.quality_tier // "unsigned_local"' "${PLUGIN_JSON}")"

if [[ -z "${PACKAGE_URL}" || -z "${EXPECTED_SHA}" || -z "${SELECTED_VERSION}" ]]; then
  echo "Invalid index entry: package_url, version, and sha256 are required" >&2
  exit 1
fi

if [[ "$(tier_rank "${QUALITY_TIER}")" -lt "$(tier_rank "${MIN_QUALITY_TIER}")" ]]; then
  echo "Plugin quality tier '${QUALITY_TIER}' is below required minimum '${MIN_QUALITY_TIER}'" >&2
  exit 1
fi

PACKAGE_PATH="${WORK_DIR}/plugin.tar.gz"
echo "[plugin-index-install] downloading plugin package: ${PACKAGE_URL}"
curl -fsSL "${PACKAGE_URL}" -o "${PACKAGE_PATH}"

ACTUAL_SHA="$(sha256_file "${PACKAGE_PATH}")"
if [[ "${ACTUAL_SHA}" != "${EXPECTED_SHA}" ]]; then
  echo "Checksum mismatch for downloaded package." >&2
  echo "  expected: ${EXPECTED_SHA}" >&2
  echo "  actual:   ${ACTUAL_SHA}" >&2
  exit 1
fi

mkdir -p "${PLUGIN_HOME}"
mkdir -p "$(dirname "${TRUST_POLICY_PATH}")"

if [[ -n "${TRUST_POLICY_URL}" ]]; then
  TRUST_TMP="${WORK_DIR}/trust-policy.json"
  echo "[plugin-index-install] fetching trust policy: ${TRUST_POLICY_URL}"
  curl -fsSL "${TRUST_POLICY_URL}" -o "${TRUST_TMP}"
  if [[ ! -f "${TRUST_POLICY_PATH}" ]]; then
    cp "${TRUST_TMP}" "${TRUST_POLICY_PATH}"
  else
    MERGED="${WORK_DIR}/trust-policy-merged.json"
    jq -s '
      (.[0] // {"require_signature": true, "publishers": []}) as $base
      | (.[1] // {"require_signature": true, "publishers": []}) as $incoming
      | {
          require_signature: (($base.require_signature // true) and ($incoming.require_signature // true)),
          publishers: (
            ([($base.publishers // []), ($incoming.publishers // [])] | add)
            | group_by(.id)
            | map(.[-1])
          )
        }
    ' "${TRUST_POLICY_PATH}" "${TRUST_TMP}" > "${MERGED}"
    mv "${MERGED}" "${TRUST_POLICY_PATH}"
  fi
fi

INSTALL_ARGS=(--package "${PACKAGE_PATH}" --plugin-home "${PLUGIN_HOME}")
if [[ "${FORCE}" == "1" ]]; then
  INSTALL_ARGS+=(--force)
fi

echo "[plugin-index-install] installing plugin id=${PLUGIN_ID} version=${SELECTED_VERSION}"
"${ROOT_DIR}/scripts/plugin-install.sh" "${INSTALL_ARGS[@]}"

cat <<EOF
[plugin-index-install] success
Use these env vars for runtime:
  KELVIN_PLUGIN_HOME=${PLUGIN_HOME}
  KELVIN_TRUST_POLICY_PATH=${TRUST_POLICY_PATH}
EOF
