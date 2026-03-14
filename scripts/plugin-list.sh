#!/usr/bin/env bash
set -euo pipefail

PLUGIN_HOME_DEFAULT="${HOME}/.kelvinclaw/plugins"
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${PLUGIN_HOME_DEFAULT}}"
OUTPUT_FORMAT="table"

usage() {
  cat <<'USAGE'
Usage: scripts/plugin-list.sh [options]

List installed Kelvin SDK plugins.

Options:
  --plugin-home <dir>  Plugin install root (default: $KELVIN_PLUGIN_HOME or ~/.kelvinclaw/plugins)
  --json               Output JSON
  --table              Output table (default)
  -h, --help           Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --plugin-home)
      PLUGIN_HOME="${2:?missing value for --plugin-home}"
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

sort_plugins_json() {
  jq -c '
    def version_key:
      (.version // "0.0.0")
      | split("+")[0]
      | split("-")[0]
      | split(".")
      | map(tonumber? // 0);
    sort_by(.id)
    | group_by(.id)
    | map(sort_by(version_key) | reverse)
    | add // []
  '
}

if [[ ! -d "${PLUGIN_HOME}" ]]; then
  if [[ "${OUTPUT_FORMAT}" == "json" ]]; then
    echo "[]"
  else
    echo "No plugins installed (${PLUGIN_HOME} does not exist)."
  fi
  exit 0
fi

collect_plugins_json() {
  local first="1"
  printf '['

  shopt -s nullglob
  for plugin_dir in "${PLUGIN_HOME}"/*; do
    [[ -d "${plugin_dir}" ]] || continue
    local plugin_id
    plugin_id="$(basename "${plugin_dir}")"
    [[ "${plugin_id}" == "current" ]] && continue

    local current=""
    if [[ -L "${plugin_dir}/current" ]]; then
      current="$(readlink "${plugin_dir}/current" || true)"
    fi

    for version_dir in "${plugin_dir}"/*; do
      [[ -d "${version_dir}" ]] || continue
      local version
      version="$(basename "${version_dir}")"
      [[ "${version}" == "current" ]] && continue

      local manifest="${version_dir}/plugin.json"
      local name=""
      local api_version=""
      if [[ -f "${manifest}" ]]; then
        name="$(jq -er '.name // ""' "${manifest}" 2>/dev/null || true)"
        api_version="$(jq -er '.api_version // ""' "${manifest}" 2>/dev/null || true)"
      fi

      [[ "${first}" == "1" ]] || printf ','
      first="0"
      jq -cn \
        --arg id "${plugin_id}" \
        --arg version "${version}" \
        --arg current "${current}" \
        --arg name "${name}" \
        --arg api_version "${api_version}" \
        '{
          id: $id,
          version: $version,
          is_current: ($version == $current),
          name: (if $name == "" then null else $name end),
          api_version: (if $api_version == "" then null else $api_version end)
        }'
    done
  done
  shopt -u nullglob

  printf ']'
}

plugins_json="$(collect_plugins_json)"

if [[ "${OUTPUT_FORMAT}" == "json" ]]; then
  sort_plugins_json <<< "${plugins_json}"
  exit 0
fi

count="$(jq 'length' <<< "${plugins_json}")"
if [[ "${count}" == "0" ]]; then
  echo "No plugins installed."
  exit 0
fi

printf "%-24s %-12s %-8s %-24s %s\n" "ID" "VERSION" "CURRENT" "API_VERSION" "NAME"
sort_plugins_json <<< "${plugins_json}" | jq -r '
  .[] |
  [
    .id,
    .version,
    (if .is_current then "yes" else "no" end),
    (.api_version // "-"),
    (.name // "-")
  ] | @tsv
' | while IFS=$'\t' read -r id version current api_version name; do
  printf "%-24s %-12s %-8s %-24s %s\n" "${id}" "${version}" "${current}" "${api_version}" "${name}"
done
