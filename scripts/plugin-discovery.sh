#!/usr/bin/env bash
set -euo pipefail

DEFAULT_INDEX_URL="https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json"
INDEX_URL="${KELVIN_PLUGIN_INDEX_URL:-${DEFAULT_INDEX_URL}}"
PLUGIN_ID=""
OUTPUT_JSON="0"

usage() {
  cat <<'USAGE'
Usage: scripts/plugin-discovery.sh [options]

Query plugin registry index metadata/discovery endpoints.

Options:
  --index-url <url>   Registry index URL (default: kelvinclaw-plugins index.json)
  --plugin <id>       Filter by plugin id
  --json              Emit JSON output
  -h, --help          Show help
USAGE
}

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 1
  fi
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
    --json)
      OUTPUT_JSON="1"
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

require_cmd curl
require_cmd jq

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "${WORK_DIR}"' EXIT
INDEX_PATH="${WORK_DIR}/index.json"

curl -fsSL "${INDEX_URL}" -o "${INDEX_PATH}"
if [[ "$(jq -r '.schema_version // "missing"' "${INDEX_PATH}")" != "v1" ]]; then
  echo "Invalid index schema_version" >&2
  exit 1
fi

if [[ -n "${PLUGIN_ID}" ]]; then
  FILTER='.plugins | map(select(.id == $id))'
  PLUGINS_JSON="$(jq -c --arg id "${PLUGIN_ID}" "${FILTER}" "${INDEX_PATH}")"
else
  PLUGINS_JSON="$(jq -c '.plugins // []' "${INDEX_PATH}")"
fi

if [[ "${OUTPUT_JSON}" == "1" ]]; then
  echo "${PLUGINS_JSON}" | jq -c 'sort_by(.id, .version)'
  exit 0
fi

COUNT="$(echo "${PLUGINS_JSON}" | jq 'length')"
if [[ "${COUNT}" == "0" ]]; then
  echo "No plugin entries found."
  exit 0
fi

printf "%-30s %-12s %-16s %s\n" "ID" "VERSION" "QUALITY_TIER" "PACKAGE_URL"
echo "${PLUGINS_JSON}" | jq -r '
  sort_by(.id, .version)[] |
  [
    (.id // "-"),
    (.version // "-"),
    (.quality_tier // "unsigned_local"),
    (.package_url // "-")
  ] | @tsv
' | while IFS=$'\t' read -r id version tier package_url; do
  printf "%-30s %-12s %-16s %s\n" "${id}" "${version}" "${tier}" "${package_url}"
done
