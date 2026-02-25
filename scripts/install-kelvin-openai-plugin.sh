#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_PATH=""
PLUGIN_SOURCE_DIR="${ROOT_DIR}/plugins/kelvin-openai"
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${ROOT_DIR}/.kelvin/plugins}"
TRUST_POLICY_PATH="${KELVIN_TRUST_POLICY_PATH:-${ROOT_DIR}/.kelvin/trusted_publishers.json}"
TRUST_POLICY_SEED="${ROOT_DIR}/plugins/trusted_publishers.kelvin.json"
FORCE="0"
REFRESH_TRUST_POLICY="0"
WORK_DIR=""

usage() {
  cat <<'USAGE'
Usage: scripts/install-kelvin-openai-plugin.sh [options]

Install Kelvin's first-party OpenAI WASM model plugin using the same package flow as third-party plugins.

Options:
  --package <path>            Plugin package path (optional)
  --plugin-source <dir>       First-party plugin source dir (default: ./plugins/kelvin-openai)
  --plugin-home <dir>         Plugin install root (default: $KELVIN_PLUGIN_HOME or ./.kelvin/plugins)
  --trust-policy-path <path>  Trust policy file path (default: $KELVIN_TRUST_POLICY_PATH or ./.kelvin/trusted_publishers.json)
  --force                     Reinstall plugin version if it already exists
  --refresh-trust-policy      Overwrite trust policy file with bundled Kelvin policy
  -h, --help                  Show help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --package)
      PACKAGE_PATH="${2:?missing value for --package}"
      shift 2
      ;;
    --plugin-source)
      PLUGIN_SOURCE_DIR="${2:?missing value for --plugin-source}"
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
    --refresh-trust-policy)
      REFRESH_TRUST_POLICY="1"
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

require_cmd jq
require_cmd tar

if [[ ! -f "${TRUST_POLICY_SEED}" ]]; then
  echo "Bundled trust policy not found: ${TRUST_POLICY_SEED}" >&2
  exit 1
fi

cleanup() {
  if [[ -n "${WORK_DIR}" && -d "${WORK_DIR}" ]]; then
    rm -rf "${WORK_DIR}"
  fi
}
trap cleanup EXIT

if [[ -z "${PACKAGE_PATH}" ]]; then
  if [[ ! -f "${PLUGIN_SOURCE_DIR}/plugin.json" ]]; then
    echo "Plugin source missing plugin.json: ${PLUGIN_SOURCE_DIR}/plugin.json" >&2
    exit 1
  fi
  if [[ ! -d "${PLUGIN_SOURCE_DIR}/payload" ]]; then
    echo "Plugin source missing payload/: ${PLUGIN_SOURCE_DIR}/payload" >&2
    exit 1
  fi
  if [[ ! -f "${PLUGIN_SOURCE_DIR}/plugin.sig" ]]; then
    echo "Plugin source missing plugin.sig: ${PLUGIN_SOURCE_DIR}/plugin.sig" >&2
    exit 1
  fi

  WORK_DIR="$(mktemp -d)"
  PACKAGE_PATH="${WORK_DIR}/kelvin.openai-0.1.0.tar.gz"
  tar -czf "${PACKAGE_PATH}" -C "${PLUGIN_SOURCE_DIR}" plugin.json payload plugin.sig
fi

if [[ ! -f "${PACKAGE_PATH}" ]]; then
  echo "Kelvin OpenAI plugin package not found: ${PACKAGE_PATH}" >&2
  exit 1
fi

mkdir -p "${PLUGIN_HOME}"
mkdir -p "$(dirname "${TRUST_POLICY_PATH}")"

if [[ ! -f "${TRUST_POLICY_PATH}" || "${REFRESH_TRUST_POLICY}" == "1" ]]; then
  cp "${TRUST_POLICY_SEED}" "${TRUST_POLICY_PATH}"
  echo "Wrote trust policy: ${TRUST_POLICY_PATH}"
else
  if ! jq -e '.publishers[]? | select(.id == "kelvin_openai")' "${TRUST_POLICY_PATH}" >/dev/null; then
    merged="$(mktemp)"
    jq -s '
      (.[0] // {"require_signature": true, "publishers": []}) as $base
      | (.[1] // {"publishers": []}) as $seed
      | {
          require_signature: ($base.require_signature // true),
          publishers: (
            ([($base.publishers // []), ($seed.publishers // [])] | add)
            | group_by(.id)
            | map(.[-1])
          )
        }
    ' "${TRUST_POLICY_PATH}" "${TRUST_POLICY_SEED}" > "${merged}"
    mv "${merged}" "${TRUST_POLICY_PATH}"
    echo "Updated trust policy with Kelvin OpenAI publisher key: ${TRUST_POLICY_PATH}"
  fi
fi

VERSION_DIR="${PLUGIN_HOME}/kelvin.openai/0.1.0"
if [[ -d "${VERSION_DIR}" && "${FORCE}" != "1" ]]; then
  echo "Kelvin OpenAI plugin already installed: ${VERSION_DIR}"
else
  INSTALL_ARGS=(--package "${PACKAGE_PATH}" --plugin-home "${PLUGIN_HOME}")
  if [[ "${FORCE}" == "1" ]]; then
    INSTALL_ARGS+=(--force)
  fi
  "${ROOT_DIR}/scripts/plugin-install.sh" "${INSTALL_ARGS[@]}"
fi

cat <<EOF2
Kelvin OpenAI plugin installation complete.
Use these env vars for runtime:
  KELVIN_PLUGIN_HOME=${PLUGIN_HOME}
  KELVIN_TRUST_POLICY_PATH=${TRUST_POLICY_PATH}
  OPENAI_API_KEY=<required>
EOF2
