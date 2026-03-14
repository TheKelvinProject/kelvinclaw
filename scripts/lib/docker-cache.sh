#!/usr/bin/env bash

kelvin_docker_cache_root() {
  local root_dir="$1"
  printf '%s' "${KELVIN_DOCKER_CACHE_ROOT:-${root_dir}/.cache/docker}"
}

kelvin_docker_buildx_cache_dir() {
  local root_dir="$1"
  local scope="${2:-default}"
  local cache_root
  cache_root="$(kelvin_docker_cache_root "${root_dir}")"
  printf '%s' "${cache_root}/buildx/${scope}"
}

kelvin_sanitize_cache_key() {
  local raw="$1"
  local sanitized
  sanitized="$(printf '%s' "${raw}" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9._-' '-')"
  sanitized="${sanitized#-}"
  sanitized="${sanitized%-}"
  if [[ -z "${sanitized}" ]]; then
    sanitized="default"
  fi
  printf '%s' "${sanitized}"
}

kelvin_prepare_docker_rust_cache() {
  local root_dir="$1"
  local cache_key="${2:-default}"
  local cache_root
  local safe_key
  local cargo_registry
  local cargo_git
  local cargo_target

  cache_root="$(kelvin_docker_cache_root "${root_dir}")"
  safe_key="$(kelvin_sanitize_cache_key "${cache_key}")"
  cargo_registry="${cache_root}/cargo/registry"
  cargo_git="${cache_root}/cargo/git"
  cargo_target="${cache_root}/target/${safe_key}"

  mkdir -p "${cargo_registry}" "${cargo_git}" "${cargo_target}"

  DOCKER_RUST_CACHE_ARGS=(
    -e "CARGO_HOME=/usr/local/cargo"
    -e "CARGO_TARGET_DIR=/kelvin-cargo-target"
    -v "${cargo_registry}:/usr/local/cargo/registry"
    -v "${cargo_git}:/usr/local/cargo/git"
    -v "${cargo_target}:/kelvin-cargo-target"
  )
}

kelvin_docker_cache_size_kb() {
  local path="$1"
  if [[ ! -e "${path}" ]]; then
    printf '0'
    return
  fi
  du -sk "${path}" 2>/dev/null | awk '{print $1}'
}

kelvin_list_cache_scope_dirs() {
  local path="$1"
  if [[ ! -d "${path}" ]]; then
    return 0
  fi
  find "${path}" -mindepth 1 -maxdepth 1 -type d -print 2>/dev/null
}
