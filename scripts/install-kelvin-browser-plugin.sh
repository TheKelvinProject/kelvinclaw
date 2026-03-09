#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_INDEX_URL="https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json"

INDEX_URL="${KELVIN_PLUGIN_INDEX_URL:-${DEFAULT_INDEX_URL}}"
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${ROOT_DIR}/.kelvin/plugins}"
TRUST_POLICY_PATH="${KELVIN_TRUST_POLICY_PATH:-${ROOT_DIR}/.kelvin/trusted_publishers.json}"
PLUGIN_VERSION=""
FORCE="0"

usage() {
  cat <<'USAGE'
Usage: scripts/install-kelvin-browser-plugin.sh [options]

Install Kelvin's optional browser automation plugin profile from the plugin index.

Options:
  --index-url <url>           Plugin index URL
  --version <version>         Specific plugin version (default: latest from index)
  --plugin-home <dir>         Plugin install root
  --trust-policy-path <path>  Trust policy file path
  --force                     Reinstall plugin version if already present
  -h, --help                  Show help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --index-url)
      INDEX_URL="${2:?missing value for --index-url}"
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

INSTALL_ARGS=(
  --index-url "${INDEX_URL}"
  --plugin "kelvin.browser.automation"
  --plugin-home "${PLUGIN_HOME}"
  --trust-policy-path "${TRUST_POLICY_PATH}"
)
if [[ -n "${PLUGIN_VERSION}" ]]; then
  INSTALL_ARGS+=(--version "${PLUGIN_VERSION}")
fi
if [[ "${FORCE}" == "1" ]]; then
  INSTALL_ARGS+=(--force)
fi

"${ROOT_DIR}/scripts/plugin-index-install.sh" "${INSTALL_ARGS[@]}"
