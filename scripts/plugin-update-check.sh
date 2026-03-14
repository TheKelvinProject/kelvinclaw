#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${HOME}/.kelvinclaw/plugins}"
INDEX_URL="${KELVIN_PLUGIN_INDEX_URL:-https://raw.githubusercontent.com/agentichighway/kelvinclaw-plugins/main/index.json}"
REGISTRY_URL="${KELVIN_PLUGIN_REGISTRY_URL:-}"
PLUGIN_ID=""
OUTPUT_FORMAT="table"

usage() {
  cat <<'USAGE'
Usage: scripts/plugin-update-check.sh [options]

Compare installed plugins against the configured plugin index or hosted registry.

Options:
  --plugin-home <dir>   Installed plugin root (default: $KELVIN_PLUGIN_HOME or ~/.kelvinclaw/plugins)
  --index-url <url>     Remote index.json URL
  --registry-url <url>  Hosted registry base URL
  --plugin <id>         Limit check to one plugin id
  --json                Emit JSON
  --table               Emit table (default)
  -h, --help            Show help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --plugin-home)
      PLUGIN_HOME="${2:?missing value for --plugin-home}"
      shift 2
      ;;
    --index-url)
      INDEX_URL="${2:?missing value for --index-url}"
      shift 2
      ;;
    --registry-url)
      REGISTRY_URL="${2:?missing value for --registry-url}"
      shift 2
      ;;
    --plugin)
      PLUGIN_ID="${2:?missing value for --plugin}"
      shift 2
      ;;
    --json)
      OUTPUT_FORMAT="json"
      shift
      ;;
    --table)
      OUTPUT_FORMAT="table"
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

if ! command -v jq >/dev/null 2>&1; then
  echo "Missing required command: jq" >&2
  exit 1
fi

list_args=(--plugin-home "${PLUGIN_HOME}" --json)
installed_json="$("${ROOT_DIR}/scripts/plugin-list.sh" "${list_args[@]}")"

discovery_args=(--json)
if [[ -n "${REGISTRY_URL}" ]]; then
  discovery_args+=(--registry-url "${REGISTRY_URL}")
else
  discovery_args+=(--index-url "${INDEX_URL}")
fi
if [[ -n "${PLUGIN_ID}" ]]; then
  discovery_args+=(--plugin "${PLUGIN_ID}")
fi
available_json="$("${ROOT_DIR}/scripts/plugin-discovery.sh" "${discovery_args[@]}")"

updates_json="$(jq -cn \
  --arg plugin_id "${PLUGIN_ID}" \
  --argjson installed "${installed_json}" \
  --argjson available "${available_json}" '
  def version_key:
    (.version // "0.0.0")
    | split("+")[0]
    | split("-")[0]
    | split(".")
    | map(tonumber? // 0);
  def latest_for($id):
    ($available | map(select(.id == $id)) | sort_by(version_key) | reverse | .[0]);
  ($installed
    | map(select(($plugin_id == "") or (.id == $plugin_id)))
    | sort_by(.id)
    | group_by(.id)
    | map(sort_by(version_key) | reverse | .[0])) as $current
  | [$current[] | . as $item | (latest_for($item.id)) as $latest | {
      id: $item.id,
      installed_version: $item.version,
      latest_version: ($latest.version // null),
      update_available: (($latest.version // "") != "" and $latest.version != $item.version),
      quality_tier: ($latest.quality_tier // null),
      package_url: ($latest.package_url // null)
    }]
')"

if [[ "${OUTPUT_FORMAT}" == "json" ]]; then
  echo "${updates_json}" | jq -c '.'
  exit 0
fi

count="$(echo "${updates_json}" | jq 'length')"
if [[ "${count}" == "0" ]]; then
  echo "No installed plugins matched the update check."
  exit 0
fi

printf "%-28s %-18s %-18s %-8s %s\n" "ID" "INSTALLED" "LATEST" "UPDATE" "QUALITY_TIER"
echo "${updates_json}" | jq -r '
  sort_by(.id)[] |
  [
    .id,
    (.installed_version // "-"),
    (.latest_version // "-"),
    (if .update_available then "yes" else "no" end),
    (.quality_tier // "-")
  ] | @tsv
' | while IFS=$'\t' read -r id installed latest update quality_tier; do
  printf "%-28s %-18s %-18s %-8s %s\n" "${id}" "${installed}" "${latest}" "${update}" "${quality_tier}"
done
