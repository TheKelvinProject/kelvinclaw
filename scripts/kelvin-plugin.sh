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

create_tar_gz() {
  local output_path="$1"
  local base_dir="$2"
  shift 2
  local stage_dir=""
  local rel_path=""
  local src_path=""

  local -a tar_args=(--format ustar -czf "${output_path}")
  if tar --help 2>/dev/null | grep -q -- '--sort='; then
    tar_args=(--sort=name --mtime='UTC 1970-01-01' --owner=0 --group=0 --numeric-owner "${tar_args[@]}")
  fi
  if tar --help 2>/dev/null | grep -q -- '--no-xattrs'; then
    tar_args=(--no-xattrs "${tar_args[@]}")
  fi
  if tar --help 2>/dev/null | grep -q -- '--no-acls'; then
    tar_args=(--no-acls "${tar_args[@]}")
  fi
  if tar --help 2>/dev/null | grep -q -- '--no-selinux'; then
    tar_args=(--no-selinux "${tar_args[@]}")
  fi

  stage_dir="$(mktemp -d)"
  for rel_path in "$@"; do
    src_path="${base_dir}/${rel_path}"
    mkdir -p "${stage_dir}/$(dirname "${rel_path}")"
    if [[ -d "${src_path}" ]]; then
      cp -R "${src_path}" "${stage_dir}/${rel_path}"
    else
      cp -p "${src_path}" "${stage_dir}/${rel_path}"
    fi
  done
  if command -v xattr >/dev/null 2>&1; then
    xattr -rc "${stage_dir}" >/dev/null 2>&1 || true
  fi

  COPYFILE_DISABLE=1 COPY_EXTENDED_ATTRIBUTES_DISABLE=1 tar "${tar_args[@]}" -C "${stage_dir}" "$@"
  rm -rf "${stage_dir}"
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

builtin_provider_profile_json() {
  case "$1" in
    openai.responses)
      printf '%s\n' '{
  "id": "openai.responses",
  "provider_name": "openai",
  "protocol_family": "openai_responses",
  "api_key_env": "OPENAI_API_KEY",
  "base_url_env": "OPENAI_BASE_URL",
  "default_base_url": "https://api.openai.com",
  "endpoint_path": "v1/responses",
  "auth_header": "authorization",
  "auth_scheme": "bearer",
  "static_headers": [],
  "default_allow_hosts": ["api.openai.com"]
}'
      ;;
    anthropic.messages)
      printf '%s\n' '{
  "id": "anthropic.messages",
  "provider_name": "anthropic",
  "protocol_family": "anthropic_messages",
  "api_key_env": "ANTHROPIC_API_KEY",
  "base_url_env": "ANTHROPIC_BASE_URL",
  "default_base_url": "https://api.anthropic.com",
  "endpoint_path": "v1/messages",
  "auth_header": "x-api-key",
  "auth_scheme": "raw",
  "static_headers": [
    {
      "name": "anthropic-version",
      "value": "2023-06-01"
    }
  ],
  "default_allow_hosts": ["api.anthropic.com"]
}'
      ;;
    *)
      return 1
      ;;
  esac
}

provider_profile_default_provider_name() {
  case "$1" in
    openai.responses) printf '%s' "openai" ;;
    anthropic.messages) printf '%s' "anthropic" ;;
    *) return 1 ;;
  esac
}

protocol_family_default_model_name() {
  local protocol_family="$1"
  local provider_name="${2:-}"
  case "${protocol_family}" in
    openai_responses) printf '%s' "gpt-4.1-mini" ;;
    anthropic_messages) printf '%s' "claude-haiku-4-5-20251001" ;;
    openai_chat_completions)
      if [[ "${provider_name}" == "openrouter" ]]; then
        printf '%s' "openai/gpt-4.1-mini"
      else
        printf '%s' "default"
      fi
      ;;
    *)
      printf '%s' "default"
      ;;
  esac
}

scaffold_model_plugin_project() {
  local output_dir="$1"
  local plugin_id="$2"
  local display_name="$3"
  local plugin_version="$4"
  local entrypoint_rel="$5"
  local crate_package_name="$6"
  local crate_lib_name="$7"

  mkdir -p "${output_dir}/src" "${output_dir}/payload"

  cat > "${output_dir}/Cargo.toml" <<EOF
[package]
name = "${crate_package_name}"
version = "${plugin_version}"
edition = "2021"
publish = false

[lib]
name = "${crate_lib_name}"
crate-type = ["cdylib"]

[workspace]
EOF

  cat > "${output_dir}/src/lib.rs" <<'EOF'
#![no_std]

#[link(wasm_import_module = "kelvin_model_host_v1")]
extern "C" {
    fn provider_profile_call(req_ptr: i32, req_len: i32) -> i64;
}

const HEAP_SIZE: usize = 1024 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut NEXT_OFFSET: usize = 0;

#[no_mangle]
pub extern "C" fn alloc(len: i32) -> i32 {
    if len <= 0 {
        return 0;
    }

    let len = len as usize;
    let align = 8usize;

    unsafe {
        let start = (NEXT_OFFSET + (align - 1)) & !(align - 1);
        let Some(end) = start.checked_add(len) else {
            return 0;
        };
        if end > HEAP_SIZE {
            return 0;
        }
        NEXT_OFFSET = end;
        core::ptr::addr_of_mut!(HEAP).cast::<u8>().add(start) as usize as i32
    }
}

#[no_mangle]
pub extern "C" fn dealloc(_ptr: i32, _len: i32) {}

#[no_mangle]
pub extern "C" fn infer(req_ptr: i32, req_len: i32) -> i64 {
    // SAFETY: The trusted Kelvin host provides this import for approved
    // provider_profile-backed model plugins.
    unsafe { provider_profile_call(req_ptr, req_len) }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
EOF

  cat > "${output_dir}/build.sh" <<EOF
#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_JSON="\${ROOT_DIR}/plugin.json"
PAYLOAD_DIR="\${ROOT_DIR}/payload"
ENTRYPOINT_REL="\$(jq -er '.entrypoint' "\${PLUGIN_JSON}")"
ENTRYPOINT_ABS="\${PAYLOAD_DIR}/\${ENTRYPOINT_REL}"
TARGET_ROOT="\${CARGO_TARGET_DIR:-\${ROOT_DIR}/target}"
TARGET_DIR="\${TARGET_ROOT}/wasm32-unknown-unknown/release"
WASM_SOURCE="\${TARGET_DIR}/${crate_lib_name}.wasm"

require_cmd() {
  local name="\$1"
  if ! command -v "\${name}" >/dev/null 2>&1; then
    echo "Missing required command: \${name}" >&2
    exit 1
  fi
}

sha256_file() {
  local file="\$1"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "\${file}" | awk '{print \$1}'
    return
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "\${file}" | awk '{print \$1}'
    return
  fi
  echo "Missing required command: shasum or sha256sum" >&2
  exit 1
}

require_cmd cargo
require_cmd jq
require_cmd rustup

rustup target add wasm32-unknown-unknown >/dev/null
cargo build --release --target wasm32-unknown-unknown

mkdir -p "\$(dirname "\${ENTRYPOINT_ABS}")"
cp "\${WASM_SOURCE}" "\${ENTRYPOINT_ABS}"

ENTRYPOINT_SHA="\$(sha256_file "\${ENTRYPOINT_ABS}")"
jq --arg sha "\${ENTRYPOINT_SHA}" '.entrypoint_sha256 = \$sha' "\${PLUGIN_JSON}" > "\${PLUGIN_JSON}.tmp"
mv "\${PLUGIN_JSON}.tmp" "\${PLUGIN_JSON}"

echo "[kelvin-plugin] built ${plugin_id} -> \${ENTRYPOINT_ABS}"
echo "[kelvin-plugin] entrypoint sha256: \${ENTRYPOINT_SHA}"
EOF
  chmod +x "${output_dir}/build.sh"

  cat > "${output_dir}/payload/README.md" <<EOF
Run ./build.sh to produce payload/${entrypoint_rel} from the Rust source in src/.
EOF

  cat > "${output_dir}/.gitignore" <<'EOF'
/dist/
/target/
/payload/*.wasm
EOF

  cat > "${output_dir}/README.md" <<EOF
# ${display_name}

Generated by \`scripts/kelvin-plugin.sh new --runtime wasm_model_v1\`.

This project uses only the public Kelvin model-plugin surface:

- \`provider_profile\` routing in \`plugin.json\`
- a tiny Rust guest compiled to \`.wasm\`
- \`kelvin plugin test|pack|verify\` for local validation

Quick commands:

\`\`\`bash
./build.sh
kelvin plugin test --manifest ./plugin.json
kelvin plugin pack --manifest ./plugin.json
kelvin plugin verify --package ./dist/${plugin_id}-${plugin_version}.tar.gz
\`\`\`

Local development plugins can stay \`unsigned_local\`. Kelvin will warn on install
but still allow them to load from a local plugin home.
EOF
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
  --provider-name <name>    Model runtime: provider name (default: derived from profile or id)
  --provider-profile <id>   Model runtime: provider_profile.id (default: openai.responses)
  --protocol-family <name>  Model runtime: openai_responses|openai_chat_completions|anthropic_messages
  --api-key-env <name>      Model runtime: API key environment variable
  --base-url-env <name>     Model runtime: base URL override environment variable
  --default-base-url <url>  Model runtime: default provider base URL
  --endpoint-path <path>    Model runtime: relative endpoint path (example: v1/responses)
  --auth-header <name>      Model runtime: auth header name (default: authorization)
  --auth-scheme <name>      Model runtime: bearer|raw (default: bearer)
  --allow-host <host>       Model runtime: allowed host pattern (repeatable)
  --model-name <name>       Model runtime: model name (default: protocol-family default)
  --entrypoint <path>       Relative wasm payload path (default: plugin.wasm)
  --quality-tier <tier>     unsigned_local|signed_community|signed_trusted (default: unsigned_local)

`wasm_model_v1` scaffolds emit a structured `provider_profile` object, create a
Rust guest source project, and run a local build, so `cargo`, `rustup`, and
`jq` must be available.
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
    jq -e '
      ((.provider_name == null) or (.provider_name | type=="string" and length>0)) and
      (.provider_profile | type=="object") and
      (.provider_profile.id | type=="string" and length>0) and
      (.provider_profile.provider_name | type=="string" and length>0) and
      (.provider_profile.protocol_family | type=="string" and (. == "openai_responses" or . == "openai_chat_completions" or . == "anthropic_messages")) and
      (.provider_profile.api_key_env | type=="string" and length>0) and
      (.provider_profile.base_url_env | type=="string" and length>0) and
      (.provider_profile.default_base_url | type=="string" and length>0) and
      (.provider_profile.endpoint_path | type=="string" and length>0) and
      (.provider_profile.auth_header | type=="string" and length>0) and
      (.provider_profile.auth_scheme | type=="string" and (. == "bearer" or . == "raw")) and
      (.provider_profile.static_headers | type=="array") and
      ([.provider_profile.static_headers[]? | (.name | type=="string" and length>0) and (.value | type=="string" and length>0)] | all) and
      (.provider_profile.default_allow_hosts | type=="array" and length>0) and
      ([.provider_profile.default_allow_hosts[] | type=="string" and length>0] | all)
    ' "${manifest_path}" >/dev/null || {
      echo "wasm_model_v1 requires a structured provider_profile object" >&2
      return 1
    }
    jq -e '.capability_scopes.network_allow_hosts | type=="array" and length>0 and ([.[] | type=="string" and length>0] | all)' "${manifest_path}" >/dev/null || {
      echo "wasm_model_v1 requires non-empty capability_scopes.network_allow_hosts" >&2
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
  local provider_name="" provider_profile_id="" protocol_family="" api_key_env="" base_url_env=""
  local default_base_url="" endpoint_path="" auth_header="" auth_scheme="bearer"
  local model_name="default" entrypoint="plugin.wasm" quality_tier="unsigned_local"
  local -a allow_hosts=()

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --id) id="${2:?missing value for --id}"; shift 2 ;;
      --name) name="${2:?missing value for --name}"; shift 2 ;;
      --version) version="${2:?missing value for --version}"; shift 2 ;;
      --runtime) runtime="${2:?missing value for --runtime}"; shift 2 ;;
      --out) out="${2:?missing value for --out}"; shift 2 ;;
      --tool-name) tool_name="${2:?missing value for --tool-name}"; shift 2 ;;
      --provider-name) provider_name="${2:?missing value for --provider-name}"; shift 2 ;;
      --provider-profile) provider_profile_id="${2:?missing value for --provider-profile}"; shift 2 ;;
      --protocol-family) protocol_family="${2:?missing value for --protocol-family}"; shift 2 ;;
      --api-key-env) api_key_env="${2:?missing value for --api-key-env}"; shift 2 ;;
      --base-url-env) base_url_env="${2:?missing value for --base-url-env}"; shift 2 ;;
      --default-base-url) default_base_url="${2:?missing value for --default-base-url}"; shift 2 ;;
      --endpoint-path) endpoint_path="${2:?missing value for --endpoint-path}"; shift 2 ;;
      --auth-header) auth_header="${2:?missing value for --auth-header}"; shift 2 ;;
      --auth-scheme) auth_scheme="${2:?missing value for --auth-scheme}"; shift 2 ;;
      --allow-host) allow_hosts+=("${2:?missing value for --allow-host}"); shift 2 ;;
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

  local capabilities runtime_extra network_allow_hosts timeout_ms crate_package_name crate_lib_name
  if [[ "${runtime}" == "wasm_model_v1" ]]; then
    local builtin_profile_json="" provider_profile_json=""
    if [[ -z "${provider_profile_id}" ]]; then
      provider_profile_id="openai.responses"
    fi
    builtin_profile_json="$(builtin_provider_profile_json "${provider_profile_id}" 2>/dev/null || true)"
    if [[ -n "${builtin_profile_json}" ]]; then
      [[ -n "${protocol_family}" ]] || protocol_family="$(jq -er '.protocol_family' <<< "${builtin_profile_json}")"
      [[ -n "${provider_name}" ]] || provider_name="$(jq -er '.provider_name' <<< "${builtin_profile_json}")"
      [[ -n "${api_key_env}" ]] || api_key_env="$(jq -er '.api_key_env' <<< "${builtin_profile_json}")"
      [[ -n "${base_url_env}" ]] || base_url_env="$(jq -er '.base_url_env' <<< "${builtin_profile_json}")"
      [[ -n "${default_base_url}" ]] || default_base_url="$(jq -er '.default_base_url' <<< "${builtin_profile_json}")"
      [[ -n "${endpoint_path}" ]] || endpoint_path="$(jq -er '.endpoint_path' <<< "${builtin_profile_json}")"
      [[ -n "${auth_header}" ]] || auth_header="$(jq -er '.auth_header' <<< "${builtin_profile_json}")"
      [[ "${auth_scheme}" != "bearer" ]] || auth_scheme="$(jq -er '.auth_scheme' <<< "${builtin_profile_json}")"
      if [[ "${#allow_hosts[@]}" -eq 0 ]]; then
        mapfile -t allow_hosts < <(jq -r '.default_allow_hosts[]' <<< "${builtin_profile_json}")
      fi
    fi
    [[ -n "${protocol_family}" ]] || {
      echo "wasm_model_v1 requires --protocol-family for non-builtin provider profiles" >&2
      exit 1
    }
    [[ -n "${provider_name}" ]] || provider_name="$(tr '.-' '_' <<< "${id}")"
    [[ -n "${api_key_env}" ]] || {
      echo "wasm_model_v1 requires --api-key-env" >&2
      exit 1
    }
    [[ -n "${base_url_env}" ]] || {
      echo "wasm_model_v1 requires --base-url-env" >&2
      exit 1
    }
    [[ -n "${auth_header}" ]] || auth_header="authorization"
    [[ -n "${default_base_url}" ]] || {
      echo "wasm_model_v1 requires --default-base-url" >&2
      exit 1
    }
    [[ -n "${endpoint_path}" ]] || {
      echo "wasm_model_v1 requires --endpoint-path" >&2
      exit 1
    }
    [[ "${auth_scheme}" == "bearer" || "${auth_scheme}" == "raw" ]] || {
      echo "wasm_model_v1 --auth-scheme must be bearer or raw" >&2
      exit 1
    }
    [[ "${#allow_hosts[@]}" -gt 0 ]] || {
      echo "wasm_model_v1 requires at least one --allow-host (or a builtin profile default)" >&2
      exit 1
    }
    if [[ "${model_name}" == "default" ]]; then
      model_name="$(protocol_family_default_model_name "${protocol_family}" "${provider_name}")"
    fi
    network_allow_hosts="$(printf '%s\n' "${allow_hosts[@]}" | jq -R . | jq -s .)"
    timeout_ms="5000"
    capabilities='["model_provider","network_egress"]'
    provider_profile_json="$(jq -cn \
      --arg id "${provider_profile_id}" \
      --arg provider_name "${provider_name}" \
      --arg protocol_family "${protocol_family}" \
      --arg api_key_env "${api_key_env}" \
      --arg base_url_env "${base_url_env}" \
      --arg default_base_url "${default_base_url}" \
      --arg endpoint_path "${endpoint_path}" \
      --arg auth_header "${auth_header}" \
      --arg auth_scheme "${auth_scheme}" \
      --argjson default_allow_hosts "${network_allow_hosts}" \
      '{
        id:$id,
        provider_name:$provider_name,
        protocol_family:$protocol_family,
        api_key_env:$api_key_env,
        base_url_env:$base_url_env,
        default_base_url:$default_base_url,
        endpoint_path:$endpoint_path,
        auth_header:$auth_header,
        auth_scheme:$auth_scheme,
        static_headers:[],
        default_allow_hosts:$default_allow_hosts
      }')"
    if [[ -n "${builtin_profile_json}" ]]; then
      provider_profile_json="$(jq -cn \
        --argjson builtin "${builtin_profile_json}" \
        --argjson overrides "${provider_profile_json}" \
        '$builtin * $overrides')"
    fi
    runtime_extra="$(jq -cn \
      --arg provider_name "${provider_name}" \
      --arg model_name "${model_name}" \
      --argjson provider_profile "${provider_profile_json}" \
      '{
        provider_name:$provider_name,
        provider_profile:$provider_profile,
        model_name:$model_name
      }')"
  else
    network_allow_hosts='[]'
    timeout_ms="2000"
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
    --argjson network_allow_hosts "${network_allow_hosts}" \
    --argjson timeout_ms "${timeout_ms}" \
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
        network_allow_hosts:$network_allow_hosts
      },
      operational_controls:{
        timeout_ms:$timeout_ms,
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

  if [[ "${runtime}" == "wasm_model_v1" ]]; then
    crate_package_name="$(tr '._' '-' <<< "${id}")-plugin"
    crate_lib_name="$(tr '.-' '_' <<< "${id}")_plugin"
    scaffold_model_plugin_project "${out}" "${id}" "${name}" "${version}" "${entrypoint}" "${crate_package_name}" "${crate_lib_name}"
    (
      cd "${out}"
      ./build.sh >/dev/null
    )
  fi

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
  create_tar_gz "${output}" "${manifest_dir}" plugin.json payload ${include_sig}
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
