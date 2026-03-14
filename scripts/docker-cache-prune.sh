#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/docker-cache.sh"

CACHE_ROOT="${KELVIN_DOCKER_CACHE_ROOT:-$(kelvin_docker_cache_root "${ROOT_DIR}")}"
MAX_AGE_DAYS="${KELVIN_DOCKER_CACHE_MAX_AGE_DAYS:-21}"
INCLUDE_CARGO="0"
DRY_RUN="0"
OUTPUT_JSON="0"

usage() {
  cat <<'USAGE'
Usage: scripts/docker-cache-prune.sh [options]

Prune stale shared Docker build/cache directories under ./.cache/docker.

Options:
  --cache-root <path>    Cache root (default: $KELVIN_DOCKER_CACHE_ROOT or ./.cache/docker)
  --max-age-days <n>     Remove scope directories older than n days (default: 21)
  --include-cargo        Also prune stale cargo registry/git cache subdirectories
  --dry-run              Show what would be removed without deleting it
  --json                 Emit JSON summary
  -h, --help             Show help
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
    --cache-root)
      CACHE_ROOT="${2:?missing value for --cache-root}"
      shift 2
      ;;
    --max-age-days)
      MAX_AGE_DAYS="${2:?missing value for --max-age-days}"
      shift 2
      ;;
    --include-cargo)
      INCLUDE_CARGO="1"
      shift
      ;;
    --dry-run)
      DRY_RUN="1"
      shift
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

require_cmd jq

if ! [[ "${MAX_AGE_DAYS}" =~ ^[0-9]+$ ]] || [[ "${MAX_AGE_DAYS}" -lt 1 ]]; then
  echo "--max-age-days must be an integer >= 1" >&2
  exit 1
fi

pruned_count=0
reclaimed_kb=0
entries_json="[]"

record_entry() {
  local scope="$1"
  local path="$2"
  local size_kb="$3"
  local action="$4"
  entries_json="$(jq -c \
    --arg scope "${scope}" \
    --arg path "${path}" \
    --argjson size_kb "${size_kb}" \
    --arg action "${action}" \
    '. + [{scope: $scope, path: $path, size_kb: $size_kb, action: $action}]' \
    <<< "${entries_json}")"
}

prune_scope() {
  local scope="$1"
  local scope_dir="$2"
  if [[ ! -d "${scope_dir}" ]]; then
    return
  fi
  while IFS= read -r candidate; do
    [[ -n "${candidate}" ]] || continue
    local size_kb
    size_kb="$(kelvin_docker_cache_size_kb "${candidate}")"
    reclaimed_kb=$((reclaimed_kb + size_kb))
    pruned_count=$((pruned_count + 1))
    if [[ "${DRY_RUN}" == "1" ]]; then
      record_entry "${scope}" "${candidate}" "${size_kb}" "would_remove"
    else
      rm -rf "${candidate}"
      record_entry "${scope}" "${candidate}" "${size_kb}" "removed"
    fi
  done < <(find "${scope_dir}" -mindepth 1 -maxdepth 1 -type d -mtime "+${MAX_AGE_DAYS}" -print 2>/dev/null)
}

prune_scope "buildx" "${CACHE_ROOT}/buildx"
prune_scope "target" "${CACHE_ROOT}/target"
if [[ "${INCLUDE_CARGO}" == "1" ]]; then
  prune_scope "cargo-registry" "${CACHE_ROOT}/cargo/registry"
  prune_scope "cargo-git" "${CACHE_ROOT}/cargo/git"
fi

if [[ "${OUTPUT_JSON}" == "1" ]]; then
  jq -cn \
    --arg cache_root "${CACHE_ROOT}" \
    --argjson max_age_days "${MAX_AGE_DAYS}" \
    --argjson dry_run "$( [[ "${DRY_RUN}" == "1" ]] && echo true || echo false )" \
    --argjson include_cargo "$( [[ "${INCLUDE_CARGO}" == "1" ]] && echo true || echo false )" \
    --argjson pruned_count "${pruned_count}" \
    --argjson reclaimed_kb "${reclaimed_kb}" \
    --argjson entries "${entries_json}" \
    '{
      cache_root: $cache_root,
      max_age_days: $max_age_days,
      dry_run: $dry_run,
      include_cargo: $include_cargo,
      pruned_count: $pruned_count,
      reclaimed_kb: $reclaimed_kb,
      entries: $entries
    }'
  exit 0
fi

if [[ "${pruned_count}" -eq 0 ]]; then
  echo "[docker-cache-prune] no stale cache directories found under ${CACHE_ROOT}"
  exit 0
fi

mode="removed"
if [[ "${DRY_RUN}" == "1" ]]; then
  mode="would remove"
fi
echo "[docker-cache-prune] ${mode} ${pruned_count} directories reclaiming ${reclaimed_kb} KB under ${CACHE_ROOT}"
jq -r '.[] | "- \(.scope): \(.path) (\(.size_kb) KB, \(.action))"' <<< "${entries_json}"
