#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_CORE_VERSIONS="0.1.0"
DEFAULT_CORE_API_VERSION="1.0.0"

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 1
  fi
}

sha256_file() {
  local file="$1"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${file}" | awk '{print $1}'
    return
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${file}" | awk '{print $1}'
    return
  fi
  echo "Missing required command: shasum or sha256sum" >&2
  exit 1
}

semver_valid() {
  [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]]
}

semver_ge() {
  local left="$1"
  local right="$2"
  [[ "$(printf '%s\n%s\n' "${left}" "${right}" | sort -V | tail -n1)" == "${left}" ]]
}

semver_le() {
  local left="$1"
  local right="$2"
  [[ "$(printf '%s\n%s\n' "${left}" "${right}" | sort -V | head -n1)" == "${left}" ]]
}

quality_tier_valid() {
  case "$1" in
    unsigned_local|signed_community|signed_trusted) return 0 ;;
    *) return 1 ;;
  esac
}

usage() {
  cat <<'USAGE'
Usage: scripts/kelvin-plugin.sh <command> [options]

Commands:
  new       Create a new plugin package scaffold.
  test      Validate plugin manifest/layout and compatibility matrix.
  pack      Build a .tar.gz plugin package from manifest + payload.
  verify    Verify package integrity and policy-tier requirements.

Run with --help after any command for command-specific options.
USAGE
}

new_usage() {
  cat <<'USAGE'
Usage: scripts/kelvin-plugin.sh new [options]

Options:
  --id <plugin-id>          Required plugin id (example: acme.echo)
  --name <display-name>     Required plugin name
  --version <semver>        Plugin version (default: 0.1.0)
  --runtime <kind>          wasm_tool_v1 or wasm_model_v1 (default: wasm_tool_v1)
  --out <dir>               Output directory (default: ./plugin-<id>)
  --tool-name <name>        Tool runtime: tool name (default: derived from id)
  --provider-name <name>    Model runtime: provider name (default: derived from id)
  --provider-profile <id>   Model runtime: host-enforced provider profile id (example: openai.responses)
  --model-name <name>       Model runtime: model name (default: default)
  --entrypoint <path>       Relative wasm payload path (default: plugin.wasm)
  --quality-tier <tier>     unsigned_local|signed_community|signed_trusted (default: unsigned_local)
USAGE
}

test_usage() {
  cat <<'USAGE'
Usage: scripts/kelvin-plugin.sh test --manifest <plugin.json> [options]

Options:
  --manifest <path>         Required path to plugin.json
  --core-versions <csv>     Core versions matrix (default: 0.1.0)
  --core-api-version <semver>
                            Core API semver (default: 1.0.0)
  --json                    Emit machine-readable output JSON
USAGE
}

pack_usage() {
  cat <<'USAGE'
Usage: scripts/kelvin-plugin.sh pack --manifest <plugin.json> [options]

Options:
  --manifest <path>         Required path to plugin.json
  --output <path>           Output .tar.gz path (default: ./dist/<id>-<version>.tar.gz)
  --core-versions <csv>     Core versions matrix for pre-pack validation
USAGE
}

verify_usage() {
  cat <<'USAGE'
Usage: scripts/kelvin-plugin.sh verify [options]

Options:
  --package <path>          Plugin package tarball (.tar.gz)
  --manifest <path>         Plugin manifest path (if package is omitted)
  --trust-policy <path>     Trust policy file for signed_trusted checks
  --core-versions <csv>     Core versions matrix (default: 0.1.0)
  --json                    Emit machine-readable output JSON

Note: you must pass either --package or --manifest.
USAGE
}

validate_manifest_and_layout() {
  local manifest_path="$1"
  local core_versions_csv="$2"
  local core_api_version="$3"
  local json_output="${4:-0}"

  require_cmd jq

  if [[ ! -f "${manifest_path}" ]]; then
    echo "Manifest not found: ${manifest_path}" >&2
    return 1
  fi

  local manifest_dir
  manifest_dir="$(cd "$(dirname "${manifest_path}")" && pwd)"
  local payload_dir="${manifest_dir}/payload"
  local core_api_major
  core_api_major="$(cut -d'.' -f1 <<< "${core_api_version}")"

  local id name version api_version runtime entrypoint capability_count quality_tier
  id="$(jq -er '.id' "${manifest_path}")"
  name="$(jq -er '.name' "${manifest_path}")"
  version="$(jq -er '.version' "${manifest_path}")"
  api_version="$(jq -er '.api_version' "${manifest_path}")"
  runtime="$(jq -er '.runtime // "wasm_tool_v1"' "${manifest_path}")"
  entrypoint="$(jq -er '.entrypoint' "${manifest_path}")"
  capability_count="$(jq -er '.capabilities | length' "${manifest_path}")"
  quality_tier="$(jq -er '.quality_tier // "unsigned_local"' "${manifest_path}")"

  [[ "${id}" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || {
    echo "Invalid plugin id '${id}'" >&2
    return 1
  }
  [[ -n "${name// }" ]] || {
    echo "Plugin name must not be empty" >&2
    return 1
  }
  semver_valid "${version}" || {
    echo "Plugin version must be semver: ${version}" >&2
    return 1
  }
  semver_valid "${api_version}" || {
    echo "Plugin api_version must be semver: ${api_version}" >&2
    return 1
  }
  quality_tier_valid "${quality_tier}" || {
    echo "Invalid quality_tier '${quality_tier}'" >&2
    return 1
  }
  [[ "${capability_count}" -ge 1 ]] || {
    echo "Manifest capabilities must contain at least one value" >&2
    return 1
  }

  case "${runtime}" in
    wasm_tool_v1|wasm_model_v1) ;;
    *)
      echo "Unsupported runtime '${runtime}'" >&2
      return 1
      ;;
  esac

  if [[ "${entrypoint}" == /* || "${entrypoint}" == *".."* ]]; then
    echo "Manifest entrypoint must be a safe relative path" >&2
    return 1
  fi

  local entrypoint_abs="${payload_dir}/${entrypoint}"
  if [[ ! -f "${entrypoint_abs}" ]]; then
    echo "Entrypoint file missing: ${entrypoint_abs}" >&2
    return 1
  fi

  local expected_sha actual_sha
  expected_sha="$(jq -er '.entrypoint_sha256 // ""' "${manifest_path}")"
  if [[ -n "${expected_sha}" ]]; then
    actual_sha="$(sha256_file "${entrypoint_abs}")"
    if [[ "${actual_sha}" != "${expected_sha}" ]]; then
      echo "entrypoint_sha256 mismatch (expected=${expected_sha} actual=${actual_sha})" >&2
      return 1
    fi
  fi

  if [[ "${runtime}" == "wasm_tool_v1" ]]; then
    jq -e '.capabilities | index("tool_provider") != null' "${manifest_path}" >/dev/null || {
      echo "wasm_tool_v1 requires capability 'tool_provider'" >&2
      return 1
    }
    jq -e '.tool_name | type=="string" and length>0' "${manifest_path}" >/dev/null || {
      echo "wasm_tool_v1 requires non-empty tool_name" >&2
      return 1
    }
  fi

  if [[ "${runtime}" == "wasm_model_v1" ]]; then
    jq -e '.capabilities | index("model_provider") != null' "${manifest_path}" >/dev/null || {
      echo "wasm_model_v1 requires capability 'model_provider'" >&2
      return 1
    }
    jq -e '(.provider_name // "" | type=="string") and (.provider_profile // "" | type=="string")' "${manifest_path}" >/dev/null || {
      echo "wasm_model_v1 provider_name/provider_profile fields must be strings when present" >&2
      return 1
    }
    jq -e '((.provider_name // "") | length > 0) or ((.provider_profile // "") | length > 0)' "${manifest_path}" >/dev/null || {
      echo "wasm_model_v1 requires non-empty provider_name or provider_profile" >&2
      return 1
    }
    jq -e '.model_name | type=="string" and length>0' "${manifest_path}" >/dev/null || {
      echo "wasm_model_v1 requires non-empty model_name" >&2
      return 1
    }
  fi

  local plugin_api_major
  plugin_api_major="$(cut -d'.' -f1 <<< "${api_version}")"
  if [[ "${plugin_api_major}" != "${core_api_major}" ]]; then
    echo "api major mismatch: plugin=${plugin_api_major} core=${core_api_major}" >&2
    return 1
  fi

  local min_core max_core
  min_core="$(jq -er '.min_core_version // ""' "${manifest_path}")"
  max_core="$(jq -er '.max_core_version // ""' "${manifest_path}")"
  if [[ -n "${min_core}" ]]; then
    semver_valid "${min_core}" || {
      echo "min_core_version must be semver" >&2
      return 1
    }
  fi
  if [[ -n "${max_core}" ]]; then
    semver_valid "${max_core}" || {
      echo "max_core_version must be semver" >&2
      return 1
    }
  fi

  local compatibility="[]"
  local core_version
  IFS=',' read -r -a _versions <<< "${core_versions_csv}"
  for core_version in "${_versions[@]}"; do
    core_version="$(xargs <<< "${core_version}")"
    [[ -n "${core_version}" ]] || continue
    semver_valid "${core_version}" || {
      echo "core version '${core_version}' is not semver" >&2
      return 1
    }
    local compatible="true"
    local reason="ok"
    if [[ -n "${min_core}" ]] && ! semver_ge "${core_version}" "${min_core}"; then
      compatible="false"
      reason="below_min_core_version"
    fi
    if [[ -n "${max_core}" ]] && ! semver_le "${core_version}" "${max_core}"; then
      compatible="false"
      reason="above_max_core_version"
    fi
    compatibility="$(
      jq -cn \
        --argjson existing "${compatibility}" \
        --arg version "${core_version}" \
        --arg compatible "${compatible}" \
        --arg reason "${reason}" \
        '$existing + [{core_version:$version, compatible:($compatible=="true"), reason:$reason}]'
    )"
  done

  if [[ "${json_output}" == "1" ]]; then
    jq -cn \
      --arg id "${id}" \
      --arg name "${name}" \
      --arg version "${version}" \
      --arg runtime "${runtime}" \
      --arg entrypoint "${entrypoint}" \
      --arg quality_tier "${quality_tier}" \
      --argjson compatibility "${compatibility}" \
      '{
        id:$id,
        name:$name,
        version:$version,
        runtime:$runtime,
        entrypoint:$entrypoint,
        quality_tier:$quality_tier,
        compatibility:$compatibility
      }'
  else
    echo "[kelvin-plugin] manifest ok: ${id}@${version} (${runtime})"
    echo "[kelvin-plugin] compatibility matrix:"
    jq -r '.[] | "  - core=\(.core_version) compatible=\(.compatible) reason=\(.reason)"' <<< "${compatibility}"
  fi
}

cmd_new() {
  local id="" name="" version="0.1.0" runtime="wasm_tool_v1" out="" tool_name=""
  local provider_name="" provider_profile="" model_name="default" entrypoint="plugin.wasm" quality_tier="unsigned_local"

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --id) id="${2:?missing value for --id}"; shift 2 ;;
      --name) name="${2:?missing value for --name}"; shift 2 ;;
      --version) version="${2:?missing value for --version}"; shift 2 ;;
      --runtime) runtime="${2:?missing value for --runtime}"; shift 2 ;;
      --out) out="${2:?missing value for --out}"; shift 2 ;;
      --tool-name) tool_name="${2:?missing value for --tool-name}"; shift 2 ;;
      --provider-name) provider_name="${2:?missing value for --provider-name}"; shift 2 ;;
      --provider-profile) provider_profile="${2:?missing value for --provider-profile}"; shift 2 ;;
      --model-name) model_name="${2:?missing value for --model-name}"; shift 2 ;;
      --entrypoint) entrypoint="${2:?missing value for --entrypoint}"; shift 2 ;;
      --quality-tier) quality_tier="${2:?missing value for --quality-tier}"; shift 2 ;;
      -h|--help) new_usage; exit 0 ;;
      *) echo "Unknown argument: $1" >&2; new_usage; exit 1 ;;
    esac
  done

  [[ -n "${id}" && -n "${name}" ]] || {
    echo "--id and --name are required" >&2
    new_usage
    exit 1
  }
  semver_valid "${version}" || {
    echo "--version must be semver" >&2
    exit 1
  }
  quality_tier_valid "${quality_tier}" || {
    echo "Invalid --quality-tier '${quality_tier}'" >&2
    exit 1
  }
  case "${runtime}" in
    wasm_tool_v1|wasm_model_v1) ;;
    *) echo "Unsupported --runtime '${runtime}'" >&2; exit 1 ;;
  esac

  if [[ -z "${out}" ]]; then
    out="./plugin-${id}"
  fi
  mkdir -p "${out}/payload"
  if [[ -z "${tool_name}" ]]; then
    tool_name="$(tr '.-' '_' <<< "${id}")"
  fi
  if [[ -z "${provider_name}" ]]; then
    case "${provider_profile}" in
      openai.responses) provider_name="openai" ;;
      anthropic.messages) provider_name="anthropic" ;;
      *) provider_name="$(tr '.-' '_' <<< "${id}")" ;;
    esac
  fi

  local capabilities runtime_extra
  if [[ "${runtime}" == "wasm_model_v1" ]]; then
    capabilities='["model_provider","network_egress"]'
    runtime_extra="$(jq -cn \
      --arg provider_name "${provider_name}" \
      --arg provider_profile "${provider_profile}" \
      --arg model_name "${model_name}" \
      '{
        provider_name:$provider_name,
        model_name:$model_name
      } + (if $provider_profile == "" then {} else {provider_profile:$provider_profile} end)')"
  else
    capabilities='["tool_provider"]'
    runtime_extra="$(jq -cn --arg tool_name "${tool_name}" '{tool_name:$tool_name}')"
  fi

  jq -cn \
    --arg id "${id}" \
    --arg name "${name}" \
    --arg version "${version}" \
    --arg runtime "${runtime}" \
    --arg entrypoint "${entrypoint}" \
    --arg quality_tier "${quality_tier}" \
    --argjson capabilities "${capabilities}" \
    --argjson runtime_extra "${runtime_extra}" \
    '{
      id:$id,
      name:$name,
      version:$version,
      api_version:"1.0.0",
      description:"Kelvin plugin scaffold",
      homepage:"https://github.com/agentichighway/kelvinclaw-plugins",
      capabilities:$capabilities,
      experimental:false,
      min_core_version:"0.1.0",
      max_core_version:null,
      runtime:$runtime,
      entrypoint:$entrypoint,
      entrypoint_sha256:null,
      publisher:null,
      quality_tier:$quality_tier,
      capability_scopes:{
        fs_read_paths:[],
        network_allow_hosts:[]
      },
      operational_controls:{
        timeout_ms:2000,
        max_retries:0,
        max_calls_per_minute:120,
        circuit_breaker_failures:3,
        circuit_breaker_cooldown_ms:30000
      }
    } + $runtime_extra' > "${out}/plugin.json"

  cat > "${out}/payload/README.md" <<'EOF'
Place your compiled WASM entrypoint file here.
Example:
  payload/plugin.wasm
EOF

  cat > "${out}/README.md" <<EOF
# ${name}

Generated by \`scripts/kelvin-plugin.sh new\`.

Quick commands:

\`\`\`bash
scripts/kelvin-plugin.sh test --manifest "${out}/plugin.json"
scripts/kelvin-plugin.sh pack --manifest "${out}/plugin.json"
\`\`\`

For signing:

\`\`\`bash
scripts/plugin-sign.sh --manifest "${out}/plugin.json" --private-key /path/to/ed25519-private.pem --publisher-id your.publisher.id --trust-policy-out "${out}/trusted_publishers.json"
\`\`\`
EOF

  echo "[kelvin-plugin] scaffold created at ${out}"
}

cmd_test() {
  local manifest="" core_versions="${DEFAULT_CORE_VERSIONS}" core_api_version="${DEFAULT_CORE_API_VERSION}" json_output="0"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --manifest) manifest="${2:?missing value for --manifest}"; shift 2 ;;
      --core-versions) core_versions="${2:?missing value for --core-versions}"; shift 2 ;;
      --core-api-version) core_api_version="${2:?missing value for --core-api-version}"; shift 2 ;;
      --json) json_output="1"; shift ;;
      -h|--help) test_usage; exit 0 ;;
      *) echo "Unknown argument: $1" >&2; test_usage; exit 1 ;;
    esac
  done
  [[ -n "${manifest}" ]] || {
    echo "--manifest is required" >&2
    test_usage
    exit 1
  }
  validate_manifest_and_layout "${manifest}" "${core_versions}" "${core_api_version}" "${json_output}"
}

cmd_pack() {
  local manifest="" output="" core_versions="${DEFAULT_CORE_VERSIONS}"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --manifest) manifest="${2:?missing value for --manifest}"; shift 2 ;;
      --output) output="${2:?missing value for --output}"; shift 2 ;;
      --core-versions) core_versions="${2:?missing value for --core-versions}"; shift 2 ;;
      -h|--help) pack_usage; exit 0 ;;
      *) echo "Unknown argument: $1" >&2; pack_usage; exit 1 ;;
    esac
  done
  [[ -n "${manifest}" ]] || {
    echo "--manifest is required" >&2
    pack_usage
    exit 1
  }
  validate_manifest_and_layout "${manifest}" "${core_versions}" "${DEFAULT_CORE_API_VERSION}" "0"

  local manifest_dir
  manifest_dir="$(cd "$(dirname "${manifest}")" && pwd)"
  local id version
  id="$(jq -er '.id' "${manifest}")"
  version="$(jq -er '.version' "${manifest}")"

  if [[ -z "${output}" ]]; then
    mkdir -p "${manifest_dir}/dist"
    output="${manifest_dir}/dist/${id}-${version}.tar.gz"
  fi
  mkdir -p "$(dirname "${output}")"

  local include_sig=""
  if [[ -f "${manifest_dir}/plugin.sig" ]]; then
    include_sig="plugin.sig"
  fi
  tar -czf "${output}" -C "${manifest_dir}" plugin.json payload ${include_sig}
  echo "[kelvin-plugin] package created: ${output}"
}

cmd_verify() {
  local package="" manifest="" trust_policy="" core_versions="${DEFAULT_CORE_VERSIONS}" json_output="0"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --package) package="${2:?missing value for --package}"; shift 2 ;;
      --manifest) manifest="${2:?missing value for --manifest}"; shift 2 ;;
      --trust-policy) trust_policy="${2:?missing value for --trust-policy}"; shift 2 ;;
      --core-versions) core_versions="${2:?missing value for --core-versions}"; shift 2 ;;
      --json) json_output="1"; shift ;;
      -h|--help) verify_usage; exit 0 ;;
      *) echo "Unknown argument: $1" >&2; verify_usage; exit 1 ;;
    esac
  done

  local work_dir
  work_dir="$(mktemp -d)"
  trap "rm -rf '${work_dir}'" EXIT

  if [[ -n "${package}" ]]; then
    [[ -f "${package}" ]] || {
      echo "Package not found: ${package}" >&2
      exit 1
    }
    tar -xzf "${package}" -C "${work_dir}"
    manifest="${work_dir}/plugin.json"
  fi
  [[ -n "${manifest}" ]] || {
    echo "Provide either --package or --manifest" >&2
    verify_usage
    exit 1
  }

  validate_manifest_and_layout "${manifest}" "${core_versions}" "${DEFAULT_CORE_API_VERSION}" "0"
  local manifest_dir quality_tier publisher sig_path
  manifest_dir="$(cd "$(dirname "${manifest}")" && pwd)"
  quality_tier="$(jq -er '.quality_tier // "unsigned_local"' "${manifest}")"
  publisher="$(jq -er '.publisher // ""' "${manifest}")"
  sig_path="${manifest_dir}/plugin.sig"

  case "${quality_tier}" in
    unsigned_local) ;;
    signed_community|signed_trusted)
      [[ -f "${sig_path}" ]] || {
        echo "quality_tier=${quality_tier} requires plugin.sig" >&2
        exit 1
      }
      [[ -n "${publisher}" ]] || {
        echo "quality_tier=${quality_tier} requires non-empty publisher" >&2
        exit 1
      }
      ;;
  esac

  if [[ "${quality_tier}" == "signed_trusted" ]]; then
    [[ -n "${trust_policy}" ]] || {
      echo "signed_trusted verification requires --trust-policy" >&2
      exit 1
    }
    [[ -f "${trust_policy}" ]] || {
      echo "Trust policy not found: ${trust_policy}" >&2
      exit 1
    }
    jq -e --arg publisher "${publisher}" '
      (.publishers // []) | any(.id == $publisher)
    ' "${trust_policy}" >/dev/null || {
      echo "publisher '${publisher}' not present in trust policy" >&2
      exit 1
    }
    jq -e --arg publisher "${publisher}" '
      ((.revoked_publishers // []) | index($publisher)) | not
    ' "${trust_policy}" >/dev/null || {
      echo "publisher '${publisher}' is revoked in trust policy" >&2
      exit 1
    }
  fi

  if [[ -n "${package}" ]]; then
    local dry_home="${work_dir}/dry-home"
    KELVIN_PLUGIN_HOME="${dry_home}" "${ROOT_DIR}/scripts/plugin-install.sh" --package "${package}" >/dev/null
  fi

  if [[ "${json_output}" == "1" ]]; then
    jq -cn \
      --arg manifest "${manifest}" \
      --arg quality_tier "${quality_tier}" \
      --arg publisher "${publisher}" \
      '{"verified":true,"manifest":$manifest,"quality_tier":$quality_tier,"publisher":(if $publisher=="" then null else $publisher end)}'
  else
    echo "[kelvin-plugin] verify ok (${quality_tier})"
  fi
}

main() {
  require_cmd jq
  require_cmd tar
  local command="${1:-}"
  if [[ -z "${command}" ]]; then
    usage
    exit 1
  fi
  shift || true

  case "${command}" in
    new) cmd_new "$@" ;;
    test) cmd_test "$@" ;;
    pack) cmd_pack "$@" ;;
    verify) cmd_verify "$@" ;;
    -h|--help) usage ;;
    *) echo "Unknown command: ${command}" >&2; usage; exit 1 ;;
  esac
}

main "$@"
