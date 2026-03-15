#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/lib/docker-cache.sh"

IMAGE_TAG="${KELVIN_PLUGIN_AUTHOR_DOCKER_IMAGE:-kelvinclaw-plugin-author:ubuntu-2404}"
DOCKERFILE="${KELVIN_PLUGIN_AUTHOR_DOCKERFILE:-${ROOT_DIR}/docker/Dockerfile.plugin-author}"
RUST_VERSION="${KELVIN_PLUGIN_AUTHOR_RUST_VERSION:-1.93.1}"
BUILDER_NAME="${KELVIN_PLUGIN_AUTHOR_BUILDER:-kelvinclaw-plugin-author}"
BUILD_CACHE_DIR="${KELVIN_PLUGIN_AUTHOR_BUILD_CACHE_DIR:-$(kelvin_docker_buildx_cache_dir "${ROOT_DIR}" "plugin-author-image")}"
WORK_DIR="/work"
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${ROOT_DIR}/.kelvin/plugins}"
TRUST_POLICY_PATH="${KELVIN_TRUST_POLICY_PATH:-${ROOT_DIR}/.kelvin/trusted_publishers.json}"

containerize_path() {
  local host_path="$1"
  if [[ "${host_path}" == "${ROOT_DIR}"* ]]; then
    printf '%s%s' "${WORK_DIR}" "${host_path#${ROOT_DIR}}"
    return
  fi
  printf '%s' "${host_path}"
}

maybe_mount_external_path() {
  local host_path="$1"
  if [[ "${host_path}" == "${ROOT_DIR}"* ]]; then
    return
  fi
  if [[ -d "${host_path}" ]]; then
    docker_args+=(-v "${host_path}:${host_path}")
    return
  fi
  local parent_dir
  parent_dir="$(dirname "${host_path}")"
  mkdir -p "${parent_dir}"
  docker_args+=(-v "${parent_dir}:${parent_dir}")
}

usage() {
  cat <<USAGE
Usage: scripts/plugin-author-docker.sh [-- <command...>]

Run Kelvin plugin authoring inside a repo-owned Ubuntu 24.04 image with a
preinstalled Rust ${RUST_VERSION} toolchain and wasm target.

Examples:
  scripts/plugin-author-docker.sh
  scripts/plugin-author-docker.sh -- scripts/test-plugin-author-kit.sh
  scripts/plugin-author-docker.sh -- bash -lc 'cd examples/kelvin-anthropic-plugin && ./build.sh'
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

command -v docker >/dev/null 2>&1 || { echo "docker is required" >&2; exit 1; }
docker buildx version >/dev/null 2>&1 || { echo "docker buildx is required" >&2; exit 1; }

if ! docker buildx inspect "${BUILDER_NAME}" >/dev/null 2>&1; then
  docker buildx create --name "${BUILDER_NAME}" --use >/dev/null
else
  docker buildx use "${BUILDER_NAME}" >/dev/null
fi

mkdir -p "${PLUGIN_HOME}" "$(dirname "${TRUST_POLICY_PATH}")" "${BUILD_CACHE_DIR}"
kelvin_prepare_docker_rust_cache "${ROOT_DIR}" "plugin-author"

CACHE_TMP="${BUILD_CACHE_DIR}-new"
rm -rf "${CACHE_TMP}"

build_cmd=(
  docker buildx build
  --builder "${BUILDER_NAME}"
  --file "${DOCKERFILE}"
  --build-arg "RUST_VERSION=${RUST_VERSION}"
  --progress plain
  --load
  --tag "${IMAGE_TAG}"
  --cache-to "type=local,dest=${CACHE_TMP},mode=max"
)
if [[ -f "${BUILD_CACHE_DIR}/index.json" ]]; then
  build_cmd+=(--cache-from "type=local,src=${BUILD_CACHE_DIR}")
fi
build_cmd+=("${ROOT_DIR}")
"${build_cmd[@]}" >/dev/null
rm -rf "${BUILD_CACHE_DIR}"
mv "${CACHE_TMP}" "${BUILD_CACHE_DIR}"

CONTAINER_PLUGIN_HOME="$(containerize_path "${PLUGIN_HOME}")"
CONTAINER_TRUST_POLICY_PATH="$(containerize_path "${TRUST_POLICY_PATH}")"

docker_args=(
  --rm
  "${DOCKER_RUST_CACHE_ARGS[@]}"
  -e DEBIAN_FRONTEND=noninteractive
  -e "KELVIN_PLUGIN_HOME=${CONTAINER_PLUGIN_HOME}"
  -e "KELVIN_TRUST_POLICY_PATH=${CONTAINER_TRUST_POLICY_PATH}"
  -v "${ROOT_DIR}:${WORK_DIR}"
  -w "${WORK_DIR}"
)

maybe_mount_external_path "${PLUGIN_HOME}"
maybe_mount_external_path "${TRUST_POLICY_PATH}"

for env_name in OPENAI_API_KEY ANTHROPIC_API_KEY KELVIN_PLUGIN_INDEX_URL; do
  [[ -n "${!env_name:-}" ]] && docker_args+=(-e "${env_name}=${!env_name}")
done

if [[ -t 0 && -t 1 ]]; then
  docker_args+=(-it)
fi

if [[ "${1:-}" == "--" ]]; then
  shift
fi

container_args=()
for arg in "$@"; do
  container_args+=("$(containerize_path "${arg}")")
done

if [[ $# -eq 0 ]]; then
  exec docker run "${docker_args[@]}" "${IMAGE_TAG}" bash
fi
exec docker run "${docker_args[@]}" "${IMAGE_TAG}" "${container_args[@]}"
