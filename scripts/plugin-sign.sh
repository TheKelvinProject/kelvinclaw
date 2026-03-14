#!/usr/bin/env bash
set -euo pipefail

MANIFEST_PATH=""
PRIVATE_KEY_PATH=""
KMS_KEY_ID=""
KMS_REGION=""
SIGNATURE_PATH=""
PUBLISHER_ID=""
TRUST_POLICY_OUT=""

usage() {
  cat <<'USAGE'
Usage: scripts/plugin-sign.sh --manifest <plugin.json> (--private-key <ed25519-private-key.pem> | --kms-key-id <key-id-or-alias>) [options]

Signs a plugin manifest and writes plugin.sig (base64 signature) for Kelvin installed-plugin verification.

Required:
  --manifest <path>        Path to plugin.json to sign
  exactly one of:
    --private-key <path>   Ed25519 private key in PEM format
    --kms-key-id <id>      AWS KMS Ed25519 key id, ARN, or alias

Optional:
  --output <path>          Signature output path (default: <manifest_dir>/plugin.sig)
  --kms-region <region>    AWS region override for KMS mode
  --publisher-id <id>      Publisher id for trust policy snippet output
  --trust-policy-out <path>Write trusted_publishers.json snippet with derived public key
  -h, --help               Show this help

Example:
  AWS_PROFILE=ah-willsarg-iam scripts/plugin-sign.sh \
    --manifest ~/.kelvinclaw/plugins/acme.echo/1.0.0/plugin.json \
    --kms-key-id alias/ah/kelvin/plugins/prod \
    --kms-region us-east-1 \
    --publisher-id acme \
    --trust-policy-out ./trusted_publishers.acme.json

  scripts/plugin-sign.sh \
    --manifest ~/.kelvinclaw/plugins/acme.echo/1.0.0/plugin.json \
    --private-key ~/.kelvinclaw/keys/acme-ed25519-private.pem \
    --publisher-id acme \
    --trust-policy-out ./trusted_publishers.acme.json
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --manifest)
      MANIFEST_PATH="${2:?missing value for --manifest}"
      shift 2
      ;;
    --private-key)
      PRIVATE_KEY_PATH="${2:?missing value for --private-key}"
      shift 2
      ;;
    --kms-key-id)
      KMS_KEY_ID="${2:?missing value for --kms-key-id}"
      shift 2
      ;;
    --kms-region)
      KMS_REGION="${2:?missing value for --kms-region}"
      shift 2
      ;;
    --output)
      SIGNATURE_PATH="${2:?missing value for --output}"
      shift 2
      ;;
    --publisher-id)
      PUBLISHER_ID="${2:?missing value for --publisher-id}"
      shift 2
      ;;
    --trust-policy-out)
      TRUST_POLICY_OUT="${2:?missing value for --trust-policy-out}"
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

require_cmd() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 1
  fi
}

require_cmd openssl
require_cmd awk
require_cmd xxd
require_cmd jq

resolve_openssl_cmd() {
  local candidate=""
  for candidate in \
    "/opt/homebrew/opt/openssl@3/bin/openssl" \
    "/usr/local/opt/openssl@3/bin/openssl"
  do
    if [[ -x "${candidate}" ]]; then
      printf '%s\n' "${candidate}"
      return 0
    fi
  done
  printf '%s\n' "openssl"
}

if [[ -z "${MANIFEST_PATH}" ]]; then
  echo "Missing required arguments." >&2
  usage
  exit 1
fi

if [[ -n "${PRIVATE_KEY_PATH}" && -n "${KMS_KEY_ID}" ]]; then
  echo "--private-key and --kms-key-id are mutually exclusive." >&2
  exit 1
fi
if [[ -z "${PRIVATE_KEY_PATH}" && -z "${KMS_KEY_ID}" ]]; then
  echo "Either --private-key or --kms-key-id is required." >&2
  exit 1
fi

if [[ ! -f "${MANIFEST_PATH}" ]]; then
  echo "Manifest not found: ${MANIFEST_PATH}" >&2
  exit 1
fi

MANIFEST_PATH="$(cd "$(dirname "${MANIFEST_PATH}")" && pwd)/$(basename "${MANIFEST_PATH}")"
if [[ -n "${PRIVATE_KEY_PATH}" ]]; then
  if [[ ! -f "${PRIVATE_KEY_PATH}" ]]; then
    echo "Private key not found: ${PRIVATE_KEY_PATH}" >&2
    exit 1
  fi
  PRIVATE_KEY_PATH="$(cd "$(dirname "${PRIVATE_KEY_PATH}")" && pwd)/$(basename "${PRIVATE_KEY_PATH}")"
fi

if [[ -n "${KMS_KEY_ID}" ]]; then
  require_cmd aws
fi

if [[ -n "${PRIVATE_KEY_PATH}" && ! -f "${PRIVATE_KEY_PATH}" ]]; then
  echo "Private key not found: ${PRIVATE_KEY_PATH}" >&2
  exit 1
fi

if [[ -z "${SIGNATURE_PATH}" ]]; then
  SIGNATURE_PATH="$(cd "$(dirname "${MANIFEST_PATH}")" && pwd)/plugin.sig"
fi

WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

OPENSSL_BIN="$(resolve_openssl_cmd)"
SIG_BIN_PATH="${WORK_DIR}/plugin.sig.bin"
PUB_PEM_PATH="${WORK_DIR}/public.pem"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KMS_PUBLIC_KEY_HELPER="${SCRIPT_DIR}/kms-ed25519-public-key.sh"

if [[ -n "${PRIVATE_KEY_PATH}" ]]; then
  # Ed25519 signs raw message bytes; plugin runtime verifies plugin.sig over plugin.json bytes.
  "${OPENSSL_BIN}" pkeyutl -sign -inkey "${PRIVATE_KEY_PATH}" -rawin -in "${MANIFEST_PATH}" -out "${SIG_BIN_PATH}"
  "${OPENSSL_BIN}" base64 -A -in "${SIG_BIN_PATH}" > "${SIGNATURE_PATH}"

  # Verify signature immediately before returning success.
  "${OPENSSL_BIN}" pkey -in "${PRIVATE_KEY_PATH}" -pubout -out "${PUB_PEM_PATH}" >/dev/null 2>&1
else
  if [[ ! -x "${KMS_PUBLIC_KEY_HELPER}" ]]; then
    echo "KMS public key helper is missing or not executable: ${KMS_PUBLIC_KEY_HELPER}" >&2
    exit 1
  fi
  AWS_ARGS=(aws)
  HELPER_ARGS=(
    --kms-key-id "${KMS_KEY_ID}"
  )
  if [[ -n "${KMS_REGION}" ]]; then
    AWS_ARGS+=(--region "${KMS_REGION}")
    HELPER_ARGS+=(--kms-region "${KMS_REGION}")
  fi
  KMS_RESPONSE="$("${AWS_ARGS[@]}" kms sign \
    --key-id "${KMS_KEY_ID}" \
    --message "fileb://${MANIFEST_PATH}" \
    --message-type RAW \
    --signing-algorithm ED25519_SHA_512 \
    --output json)"
  SIG_B64="$(printf '%s' "${KMS_RESPONSE}" | jq -er '.Signature')"
  printf '%s' "${SIG_B64}" > "${SIGNATURE_PATH}"
  printf '%s' "${SIG_B64}" | "${OPENSSL_BIN}" base64 -d -A > "${SIG_BIN_PATH}"
  "${KMS_PUBLIC_KEY_HELPER}" "${HELPER_ARGS[@]}" --format pem --output "${PUB_PEM_PATH}" >/dev/null
fi

if ! "${OPENSSL_BIN}" pkeyutl -verify -pubin -inkey "${PUB_PEM_PATH}" -rawin -in "${MANIFEST_PATH}" -sigfile "${SIG_BIN_PATH}" >/dev/null 2>&1; then
  echo "Signature verification failed after signing; refusing to continue." >&2
  exit 1
fi

echo "Wrote signature: ${SIGNATURE_PATH}"

if [[ -n "${PRIVATE_KEY_PATH}" ]]; then
  # Derive raw 32-byte Ed25519 public key and emit base64 for Kelvin trust policy.
  PUB_HEX="$(
    "${OPENSSL_BIN}" pkey -in "${PRIVATE_KEY_PATH}" -pubout -text -noout 2>/dev/null \
      | awk '
        /^pub:/ {capture=1; next}
        capture && /^[[:space:]]*$/ {capture=0; next}
        capture {gsub(/[ :]/, "", $0); printf "%s", $0}
      '
  )"

  if [[ ${#PUB_HEX} -ne 64 ]]; then
    echo "Failed to derive a raw 32-byte Ed25519 public key from private key." >&2
    exit 1
  fi

  PUB_B64="$(printf '%s' "${PUB_HEX}" | xxd -r -p | "${OPENSSL_BIN}" base64 -A)"
else
  PUB_B64="$("${KMS_PUBLIC_KEY_HELPER}" "${HELPER_ARGS[@]}" --format raw-base64)"
fi
echo "Derived publisher public key (base64): ${PUB_B64}"

if [[ -n "${TRUST_POLICY_OUT}" ]]; then
  if [[ -z "${PUBLISHER_ID}" ]]; then
    echo "--publisher-id is required when --trust-policy-out is used." >&2
    exit 1
  fi
  jq -n \
    --arg publisher_id "${PUBLISHER_ID}" \
    --arg public_key "${PUB_B64}" \
    '{
      require_signature: true,
      publishers: [
        {
          id: $publisher_id,
          ed25519_public_key: $public_key
        }
      ]
    }' > "${TRUST_POLICY_OUT}"
  echo "Wrote trust policy snippet: ${TRUST_POLICY_OUT}"
fi
