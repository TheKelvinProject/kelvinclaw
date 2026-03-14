#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -x "${SCRIPT_DIR}/bin/kelvin-host" ]]; then
  ROOT_DIR="${SCRIPT_DIR}"
else
  ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
fi
KELVIN_HOME_DEFAULT="${HOME}/.kelvinclaw"
KELVIN_HOME="${KELVIN_HOME:-${KELVIN_HOME_DEFAULT}}"
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${KELVIN_HOME}/plugins}"
TRUST_POLICY_PATH="${KELVIN_TRUST_POLICY_PATH:-${KELVIN_HOME}/trusted_publishers.json}"
STATE_DIR="${KELVIN_STATE_DIR:-${KELVIN_HOME}/state}"
DEFAULT_PROMPT="${KELVIN_DEFAULT_PROMPT:-What is KelvinClaw?}"
PLUGIN_MANIFEST_PATH="${ROOT_DIR}/share/official-first-party-plugins.env"
ENV_SEARCH_PATHS=(
  "${PWD}/.env.local"
  "${PWD}/.env"
  "${KELVIN_HOME}/.env.local"
  "${KELVIN_HOME}/.env"
)

usage() {
  cat <<'USAGE'
Usage: ./kelvin [kelvin-host args]

Release-bundle launcher for KelvinClaw.

Behavior:
  - with no args, installs required official plugins on first run
  - starts interactive mode on a TTY
  - falls back to a default prompt when stdin/stdout are not TTYs

Environment:
  KELVIN_HOME                Override bundle-managed state root (default: ~/.kelvinclaw)
  KELVIN_PLUGIN_HOME         Override plugin install root
  KELVIN_TRUST_POLICY_PATH   Override trust policy path
  KELVIN_STATE_DIR           Override host state dir
  KELVIN_DEFAULT_PROMPT      Prompt used for non-interactive no-arg runs
  OPENAI_API_KEY             If set, installs and selects kelvin.openai on first run

The launcher also reads OPENAI_API_KEY from:
  - ./.env.local
  - ./.env
  - ~/.kelvinclaw/.env.local
  - ~/.kelvinclaw/.env
USAGE
}

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 1
  fi
}

sha256_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${file}" | awk '{print $1}'
    return 0
  fi
  shasum -a 256 "${file}" | awk '{print $1}'
}

trim_whitespace() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "${value}"
}

strip_wrapping_quotes() {
  local value="$1"
  if [[ "${value}" == \"*\" && "${value}" == *\" ]]; then
    printf '%s' "${value:1:${#value}-2}"
    return
  fi
  if [[ "${value}" == \'*\' && "${value}" == *\' ]]; then
    printf '%s' "${value:1:${#value}-2}"
    return
  fi
  printf '%s' "${value}"
}

load_env_var_from_file() {
  local key="$1"
  local file="$2"
  local line=""
  local stripped=""
  local value=""
  [[ -f "${file}" ]] || return 1

  while IFS= read -r line || [[ -n "${line}" ]]; do
    stripped="$(trim_whitespace "${line%%#*}")"
    [[ -z "${stripped}" ]] && continue
    if [[ "${stripped}" =~ ^export[[:space:]]+ ]]; then
      stripped="$(trim_whitespace "${stripped#export }")"
    fi
    if [[ "${stripped}" =~ ^${key}[[:space:]]*=[[:space:]]*(.*)$ ]]; then
      value="$(trim_whitespace "${BASH_REMATCH[1]}")"
      strip_wrapping_quotes "${value}"
      return 0
    fi
  done < "${file}"

  return 1
}

load_dotenv_defaults() {
  local env_file=""
  local value=""
  [[ -n "${OPENAI_API_KEY:-}" ]] && return 0

  for env_file in "${ENV_SEARCH_PATHS[@]}"; do
    if value="$(load_env_var_from_file "OPENAI_API_KEY" "${env_file}")"; then
      export OPENAI_API_KEY="${value}"
      return 0
    fi
  done
}

prompt_for_openai_api_key() {
  local value=""
  [[ -n "${OPENAI_API_KEY:-}" ]] && return 0
  [[ $# -eq 0 ]] || return 0
  [[ -t 0 && -t 1 ]] || return 0

  echo "[kelvin] OPENAI_API_KEY not found in the environment or .env files."
  printf '[kelvin] Paste your OpenAI API key for this run, or press Enter to continue with echo mode: ' >&2
  IFS= read -r -s value
  printf '\n' >&2

  value="$(trim_whitespace "${value}")"
  if [[ -n "${value}" ]]; then
    export OPENAI_API_KEY="${value}"
  fi
}

plugin_current_version() {
  local plugin_id="$1"
  local current_link="${PLUGIN_HOME}/${plugin_id}/current"

  if [[ -L "${current_link}" ]]; then
    basename "$(readlink "${current_link}")"
    return 0
  fi
  if [[ -f "${current_link}/plugin.json" ]]; then
    awk -F'"' '/"version"[[:space:]]*:/ {print $4; exit}' "${current_link}/plugin.json"
    return 0
  fi
  return 1
}

ensure_trust_policy() {
  if [[ -f "${TRUST_POLICY_PATH}" ]]; then
    return 0
  fi
  mkdir -p "$(dirname "${TRUST_POLICY_PATH}")"
  echo "[kelvin] fetching official trust policy"
  curl -fsSL "${OFFICIAL_TRUST_POLICY_URL}" -o "${TRUST_POLICY_PATH}"
}

extract_package_cleanly() {
  local tarball_path="$1"
  local extract_dir="$2"
  local stderr_path="${extract_dir}/tar.stderr"

  mkdir -p "${extract_dir}"
  if ! tar -xzf "${tarball_path}" -C "${extract_dir}" 2>"${stderr_path}"; then
    cat "${stderr_path}" >&2 || true
    return 1
  fi

  if [[ -s "${stderr_path}" ]]; then
    if grep -Fv "Ignoring unknown extended header keyword 'LIBARCHIVE.xattr.com.apple.provenance'" "${stderr_path}" | grep -q .; then
      cat "${stderr_path}" >&2
      return 1
    fi
  fi

  find "${extract_dir}" -name '._*' -delete
  rm -f "${stderr_path}"
}

install_official_plugin() {
  local plugin_id="$1"
  local version="$2"
  local package_url="$3"
  local expected_sha="$4"
  local current_version=""
  local work_dir=""
  local package_path=""
  local install_dir=""
  local current_link=""

  current_version="$(plugin_current_version "${plugin_id}" || true)"
  if [[ "${current_version}" == "${version}" && -f "${PLUGIN_HOME}/${plugin_id}/${version}/plugin.json" ]]; then
    return 0
  fi

  echo "[kelvin] installing official plugin: ${plugin_id}@${version}"
  ensure_trust_policy
  mkdir -p "${PLUGIN_HOME}/${plugin_id}"

  work_dir="$(mktemp -d)"
  package_path="${work_dir}/plugin.tar.gz"
  curl -fsSL "${package_url}" -o "${package_path}"

  if [[ "$(sha256_file "${package_path}")" != "${expected_sha}" ]]; then
    echo "Checksum mismatch for ${plugin_id}@${version}" >&2
    rm -rf "${work_dir}"
    exit 1
  fi

  extract_package_cleanly "${package_path}" "${work_dir}/extract"
  install_dir="${PLUGIN_HOME}/${plugin_id}/${version}"
  current_link="${PLUGIN_HOME}/${plugin_id}/current"

  rm -rf "${install_dir}"
  mkdir -p "${install_dir}"
  cp -R "${work_dir}/extract/." "${install_dir}/"
  ln -sfn "${version}" "${current_link}"
  rm -rf "${work_dir}"
}

bootstrap_official_plugins() {
  require_cmd curl
  require_cmd tar
  require_cmd awk

  if [[ ! -f "${PLUGIN_MANIFEST_PATH}" ]]; then
    echo "Release bundle is missing ${PLUGIN_MANIFEST_PATH}" >&2
    exit 1
  fi
  # shellcheck disable=SC1090
  source "${PLUGIN_MANIFEST_PATH}"

  install_official_plugin "kelvin.cli" "${KELVIN_CLI_VERSION}" "${KELVIN_CLI_PACKAGE_URL}" "${KELVIN_CLI_SHA256}"

  if [[ -n "${OPENAI_API_KEY:-}" ]]; then
    install_official_plugin "kelvin.openai" "${KELVIN_OPENAI_VERSION}" "${KELVIN_OPENAI_PACKAGE_URL}" "${KELVIN_OPENAI_SHA256}"
  fi
}

if [[ $# -gt 0 ]]; then
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
  esac
fi

load_dotenv_defaults
prompt_for_openai_api_key "$@"

bootstrap_official_plugins

mkdir -p "${STATE_DIR}"
export KELVIN_PLUGIN_HOME="${PLUGIN_HOME}"
export KELVIN_TRUST_POLICY_PATH="${TRUST_POLICY_PATH}"

DEFAULT_HOST_ARGS=()
if [[ -n "${OPENAI_API_KEY:-}" ]]; then
  DEFAULT_HOST_ARGS+=(--model-provider kelvin.openai)
fi

if [[ $# -eq 0 ]]; then
  if [[ -t 0 && -t 1 ]]; then
    exec "${ROOT_DIR}/bin/kelvin-host" \
      "${DEFAULT_HOST_ARGS[@]}" \
      --interactive \
      --workspace "$(pwd)" \
      --state-dir "${STATE_DIR}"
  fi

  exec "${ROOT_DIR}/bin/kelvin-host" \
    "${DEFAULT_HOST_ARGS[@]}" \
    --prompt "${DEFAULT_PROMPT}" \
    --workspace "$(pwd)" \
    --state-dir "${STATE_DIR}"
fi

exec "${ROOT_DIR}/bin/kelvin-host" "$@"
