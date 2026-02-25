#!/usr/bin/env bash
set -euo pipefail

PLUGIN_HOME_DEFAULT="${HOME}/.kelvinclaw/plugins"
PLUGIN_HOME="${KELVIN_PLUGIN_HOME:-${PLUGIN_HOME_DEFAULT}}"

PLUGIN_ID=""
PLUGIN_VERSION=""
PURGE="0"

usage() {
  cat <<'USAGE'
Usage:
  scripts/plugin-uninstall.sh --id <plugin-id> --version <version> [options]
  scripts/plugin-uninstall.sh --id <plugin-id> --purge [options]

Uninstall a Kelvin SDK plugin version or purge all versions for a plugin.

Options:
  --id <plugin-id>      Plugin id (required)
  --version <version>   Installed version to remove
  --purge               Remove all installed versions for the plugin id
  --plugin-home <dir>   Plugin install root (default: $KELVIN_PLUGIN_HOME or ~/.kelvinclaw/plugins)
  -h, --help            Show this help

Examples:
  scripts/plugin-uninstall.sh --id acme.echo --version 1.0.0
  scripts/plugin-uninstall.sh --id acme.echo --purge
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --id)
      PLUGIN_ID="${2:?missing value for --id}"
      shift 2
      ;;
    --version)
      PLUGIN_VERSION="${2:?missing value for --version}"
      shift 2
      ;;
    --purge)
      PURGE="1"
      shift
      ;;
    --plugin-home)
      PLUGIN_HOME="${2:?missing value for --plugin-home}"
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

if [[ -z "${PLUGIN_ID}" ]]; then
  echo "Missing --id <plugin-id>" >&2
  usage
  exit 1
fi

if [[ "${PURGE}" != "1" && -z "${PLUGIN_VERSION}" ]]; then
  echo "Specify either --version <version> or --purge" >&2
  usage
  exit 1
fi

if [[ "${PURGE}" == "1" && -n "${PLUGIN_VERSION}" ]]; then
  echo "Use --purge without --version" >&2
  usage
  exit 1
fi

PLUGIN_DIR="${PLUGIN_HOME}/${PLUGIN_ID}"
CURRENT_LINK="${PLUGIN_DIR}/current"

if [[ ! -d "${PLUGIN_DIR}" ]]; then
  echo "Plugin not installed: ${PLUGIN_ID}" >&2
  exit 1
fi

if [[ "${PURGE}" == "1" ]]; then
  rm -rf "${PLUGIN_DIR}"
  echo "Purged plugin ${PLUGIN_ID}"
  exit 0
fi

VERSION_DIR="${PLUGIN_DIR}/${PLUGIN_VERSION}"
if [[ ! -d "${VERSION_DIR}" ]]; then
  echo "Plugin version not installed: ${PLUGIN_ID}@${PLUGIN_VERSION}" >&2
  exit 1
fi

rm -rf "${VERSION_DIR}"

if [[ -L "${CURRENT_LINK}" ]]; then
  current_target="$(readlink "${CURRENT_LINK}" || true)"
  if [[ "${current_target}" == "${PLUGIN_VERSION}" ]]; then
    shopt -s nullglob
    remaining=()
    for dir in "${PLUGIN_DIR}"/*; do
      [[ -d "${dir}" ]] || continue
      version="$(basename "${dir}")"
      [[ "${version}" == "current" ]] && continue
      remaining+=("${version}")
    done
    shopt -u nullglob
    if [[ "${#remaining[@]}" -gt 0 ]]; then
      IFS=$'\n' sorted=($(printf '%s\n' "${remaining[@]}" | sort))
      unset IFS
      last_index=$((${#sorted[@]} - 1))
      next="${sorted[${last_index}]}"
      ln -sfn "${next}" "${CURRENT_LINK}"
    else
      rm -f "${CURRENT_LINK}"
    fi
  fi
fi

if [[ -z "$(find "${PLUGIN_DIR}" -mindepth 1 -maxdepth 1 -type d 2>/dev/null || true)" ]]; then
  rm -f "${CURRENT_LINK}"
  rmdir "${PLUGIN_DIR}" 2>/dev/null || true
fi

echo "Uninstalled plugin ${PLUGIN_ID}@${PLUGIN_VERSION}"
