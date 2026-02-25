#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCKERFILE="${KELVIN_DOCKERFILE:-${ROOT_DIR}/docker/Dockerfile.test}"
RUST_VERSION="${KELVIN_RUST_VERSION:-1.93.1}"
LANE="${KELVIN_DOCKER_LANE:-full}" # quick | full
CLEAN="0"
FINAL="0"
PROGRESS="${KELVIN_DOCKER_PROGRESS:-plain}"
CACHE_DIR="${KELVIN_DOCKER_CACHE_DIR:-${ROOT_DIR}/.cache/docker/buildx}"
BUILDER_NAME="${KELVIN_DOCKER_BUILDER:-kelvinclaw-builder}"

usage() {
  cat <<USAGE
Usage: scripts/test-docker.sh [options]

Build/test Kelvin in Docker using pinned toolchain and cached layers.

Options:
  --lane <quick|full>   Build target lane (default: ${LANE})
  --clean               Rebuild from scratch (no cache)
  --final               Alias for --lane full --clean
  --rust <version>      Rust version build arg (default: ${RUST_VERSION})
  --progress <mode>     buildx progress mode (default: ${PROGRESS})
  -h, --help            Show help

Examples:
  scripts/test-docker.sh
  scripts/test-docker.sh --lane quick
  scripts/test-docker.sh --clean
  scripts/test-docker.sh --final
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --lane)
      LANE="${2:?missing value for --lane}"
      shift 2
      ;;
    --clean)
      CLEAN="1"
      shift
      ;;
    --final)
      FINAL="1"
      LANE="full"
      CLEAN="1"
      shift
      ;;
    --rust)
      RUST_VERSION="${2:?missing value for --rust}"
      shift 2
      ;;
    --progress)
      PROGRESS="${2:?missing value for --progress}"
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

if [[ "${LANE}" != "quick" && "${LANE}" != "full" ]]; then
  echo "Invalid lane: ${LANE} (expected quick or full)" >&2
  exit 1
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required" >&2
  exit 1
fi

if ! docker buildx version >/dev/null 2>&1; then
  echo "docker buildx is required" >&2
  exit 1
fi

if ! docker buildx inspect "${BUILDER_NAME}" >/dev/null 2>&1; then
  docker buildx create --name "${BUILDER_NAME}" --use >/dev/null
else
  docker buildx use "${BUILDER_NAME}" >/dev/null
fi

mkdir -p "${CACHE_DIR}"
CACHE_TMP="${CACHE_DIR}-new"
rm -rf "${CACHE_TMP}"

echo "[test-docker] lane=${LANE} clean=${CLEAN} rust=${RUST_VERSION}"
if [[ "${FINAL}" == "1" ]]; then
  echo "[test-docker] final-mode enabled (full + no-cache)"
fi

build_cmd=(
  docker buildx build
  --builder "${BUILDER_NAME}"
  --file "${DOCKERFILE}"
  --target "${LANE}"
  --build-arg "RUST_VERSION=${RUST_VERSION}"
  --progress "${PROGRESS}"
  --load
  --tag "kelvinclaw-test:${LANE}"
  --cache-to "type=local,dest=${CACHE_TMP},mode=max"
)

if [[ "${CLEAN}" == "1" ]]; then
  build_cmd+=(--no-cache --pull)
else
  if [[ -d "${CACHE_DIR}" ]]; then
    build_cmd+=(--cache-from "type=local,src=${CACHE_DIR}")
  fi
fi

build_cmd+=("${ROOT_DIR}")

"${build_cmd[@]}"

rm -rf "${CACHE_DIR}"
mv "${CACHE_TMP}" "${CACHE_DIR}"

echo "[test-docker] success"
