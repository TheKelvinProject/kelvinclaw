#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRACK="${KELVIN_FIRST_RUN_TRACK:-all}"
ITERATIONS="${KELVIN_FIRST_RUN_ITERATIONS:-3}"
THRESHOLD_PERCENT="${KELVIN_FIRST_RUN_THRESHOLD_PERCENT:-95}"
PROMPT_BASE="${KELVIN_FIRST_RUN_PROMPT:-kelvin first run gate}"

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "[first-run-gate] missing required command: ${name}" >&2
    exit 1
  fi
}

usage() {
  cat <<'USAGE'
Usage: scripts/first-run-success-rate.sh [options]

Measure scripted first-run success rate using onboarding verification tracks.

Options:
  --track <beginner|rust|wasm|all>  Verification track to run (default: all)
  --iterations <n>                  Number of iterations (default: 3)
  --threshold-percent <n>           Required success rate percent (default: 95)
  --prompt <text>                   Prompt prefix used during verification
  -h, --help                        Show help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --track)
      TRACK="${2:?missing value for --track}"
      shift 2
      ;;
    --iterations)
      ITERATIONS="${2:?missing value for --iterations}"
      shift 2
      ;;
    --threshold-percent)
      THRESHOLD_PERCENT="${2:?missing value for --threshold-percent}"
      shift 2
      ;;
    --prompt)
      PROMPT_BASE="${2:?missing value for --prompt}"
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

if ! [[ "${ITERATIONS}" =~ ^[0-9]+$ ]] || [[ "${ITERATIONS}" -lt 1 ]]; then
  echo "iterations must be >= 1" >&2
  exit 1
fi

require_cmd jq
if ! [[ "${THRESHOLD_PERCENT}" =~ ^[0-9]+$ ]] || [[ "${THRESHOLD_PERCENT}" -lt 1 ]] || [[ "${THRESHOLD_PERCENT}" -gt 100 ]]; then
  echo "threshold-percent must be between 1 and 100" >&2
  exit 1
fi

passes=0
failures=0
results_json="[]"
start_epoch_ms="$(date +%s000)"

for i in $(seq 1 "${ITERATIONS}"); do
  echo "[first-run-gate] iteration ${i}/${ITERATIONS} track=${TRACK}"
  iteration_prompt="${PROMPT_BASE} #${i}"
  if (cd "${ROOT_DIR}" && KELVIN_VERIFY_PROMPT="${iteration_prompt}" scripts/verify-onboarding.sh --track "${TRACK}"); then
    passes=$((passes + 1))
    results_json="$(printf '%s' "${results_json}" | jq --arg idx "${i}" '. + [{"iteration": ($idx|tonumber), "status":"pass"}]')"
  else
    failures=$((failures + 1))
    results_json="$(printf '%s' "${results_json}" | jq --arg idx "${i}" '. + [{"iteration": ($idx|tonumber), "status":"fail"}]')"
  fi
done

success_rate="$(awk "BEGIN { printf \"%.2f\", (${passes} / ${ITERATIONS}) * 100 }")"
end_epoch_ms="$(date +%s000)"

summary_json="$(jq -n \
  --arg track "${TRACK}" \
  --argjson iterations "${ITERATIONS}" \
  --argjson passes "${passes}" \
  --argjson failures "${failures}" \
  --arg success_rate "${success_rate}" \
  --argjson threshold "${THRESHOLD_PERCENT}" \
  --argjson started_at_ms "${start_epoch_ms}" \
  --argjson finished_at_ms "${end_epoch_ms}" \
  --argjson results "${results_json}" \
  '{
    track: $track,
    iterations: $iterations,
    passes: $passes,
    failures: $failures,
    success_rate_percent: ($success_rate|tonumber),
    threshold_percent: $threshold,
    started_at_ms: $started_at_ms,
    finished_at_ms: $finished_at_ms,
    gate_passed: (($success_rate|tonumber) >= $threshold),
    results: $results
  }')"

echo "${summary_json}" | jq .

if awk "BEGIN { exit !(${success_rate} >= ${THRESHOLD_PERCENT}) }"; then
  echo "[first-run-gate] PASS (${success_rate}% >= ${THRESHOLD_PERCENT}%)"
  exit 0
fi

echo "[first-run-gate] FAIL (${success_rate}% < ${THRESHOLD_PERCENT}%)" >&2
exit 1
