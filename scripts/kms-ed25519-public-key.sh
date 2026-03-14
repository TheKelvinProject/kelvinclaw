#!/usr/bin/env bash
set -euo pipefail

KMS_KEY_ID=""
KMS_REGION=""
FORMAT="pem"
PUBLISHER_ID=""
OUTPUT_PATH=""

usage() {
  cat <<'USAGE'
Usage: scripts/kms-ed25519-public-key.sh --kms-key-id <key-id-or-alias> [options]

Fetches an AWS KMS Ed25519 public key and emits it in one of the formats Kelvin uses.

Required:
  --kms-key-id <id>        KMS key id, ARN, or alias

Optional:
  --kms-region <region>    AWS region override (default: SDK/CLI resolution)
  --format <name>          Output format: pem | raw-base64 | trust-policy
  --publisher-id <id>      Publisher id required for trust-policy output
  --output <path>          Write output to a file instead of stdout
  -h, --help               Show this help

Examples:
  AWS_PROFILE=ah-willsarg-iam scripts/kms-ed25519-public-key.sh \
    --kms-key-id alias/ah/kelvin/plugins/prod \
    --kms-region us-east-1 \
    --format raw-base64

  AWS_PROFILE=ah-willsarg-iam scripts/kms-ed25519-public-key.sh \
    --kms-key-id alias/ah/kelvin/plugins/prod \
    --kms-region us-east-1 \
    --format trust-policy \
    --publisher-id kelvin_firstparty_aws_v1 \
    --output ./trusted_publishers.kelvin.json
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --kms-key-id)
      KMS_KEY_ID="${2:?missing value for --kms-key-id}"
      shift 2
      ;;
    --kms-region)
      KMS_REGION="${2:?missing value for --kms-region}"
      shift 2
      ;;
    --format)
      FORMAT="${2:?missing value for --format}"
      shift 2
      ;;
    --publisher-id)
      PUBLISHER_ID="${2:?missing value for --publisher-id}"
      shift 2
      ;;
    --output)
      OUTPUT_PATH="${2:?missing value for --output}"
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

require_cmd aws
require_cmd jq
require_cmd openssl
require_cmd awk
require_cmd xxd

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

if [[ -z "${KMS_KEY_ID}" ]]; then
  echo "Missing required arguments." >&2
  usage
  exit 1
fi

case "${FORMAT}" in
  pem|raw-base64|trust-policy)
    ;;
  *)
    echo "Unsupported format: ${FORMAT}" >&2
    exit 1
    ;;
esac

if [[ "${FORMAT}" == "trust-policy" && -z "${PUBLISHER_ID}" ]]; then
  echo "--publisher-id is required when --format trust-policy is used." >&2
  exit 1
fi

WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

OPENSSL_BIN="$(resolve_openssl_cmd)"
PUB_DER_PATH="${WORK_DIR}/public.der"
PUB_PEM_PATH="${WORK_DIR}/public.pem"

AWS_ARGS=(aws)
if [[ -n "${KMS_REGION}" ]]; then
  AWS_ARGS+=(--region "${KMS_REGION}")
fi
AWS_ARGS+=(
  kms
  get-public-key
  --key-id "${KMS_KEY_ID}"
  --output json
)

AWS_RESPONSE="$("${AWS_ARGS[@]}")"
KEY_SPEC="$(printf '%s' "${AWS_RESPONSE}" | jq -r '.KeySpec // empty')"
if [[ "${KEY_SPEC}" != "ECC_NIST_EDWARDS25519" ]]; then
  echo "KMS key '${KMS_KEY_ID}' must use KeySpec ECC_NIST_EDWARDS25519; got '${KEY_SPEC}'." >&2
  exit 1
fi

PUB_DER_B64="$(printf '%s' "${AWS_RESPONSE}" | jq -er '.PublicKey')"
printf '%s' "${PUB_DER_B64}" | "${OPENSSL_BIN}" base64 -d -A > "${PUB_DER_PATH}"
"${OPENSSL_BIN}" pkey -pubin -inform DER -in "${PUB_DER_PATH}" -outform PEM -out "${PUB_PEM_PATH}" >/dev/null 2>&1

PUB_HEX="$(
  "${OPENSSL_BIN}" pkey -pubin -inform DER -in "${PUB_DER_PATH}" -text -noout 2>/dev/null \
    | awk '
      /^pub:/ {capture=1; next}
      capture && /^[[:space:]]*$/ {capture=0; next}
      capture {gsub(/[ :]/, "", $0); printf "%s", $0}
    '
)"

if [[ ${#PUB_HEX} -ne 64 ]]; then
  echo "Failed to derive a raw 32-byte Ed25519 public key from KMS output." >&2
  exit 1
fi

PUB_RAW_B64="$(printf '%s' "${PUB_HEX}" | xxd -r -p | "${OPENSSL_BIN}" base64 -A)"

case "${FORMAT}" in
  pem)
    RESULT="$(cat "${PUB_PEM_PATH}")"
    ;;
  raw-base64)
    RESULT="${PUB_RAW_B64}"
    ;;
  trust-policy)
    RESULT="$(
      jq -n \
        --arg publisher_id "${PUBLISHER_ID}" \
        --arg public_key "${PUB_RAW_B64}" \
        '{
          require_signature: true,
          publishers: [
            {
              id: $publisher_id,
              ed25519_public_key: $public_key
            }
          ]
        }'
    )"
    ;;
esac

if [[ -n "${OUTPUT_PATH}" ]]; then
  printf '%s\n' "${RESULT}" > "${OUTPUT_PATH}"
  echo "Wrote ${FORMAT} output: ${OUTPUT_PATH}"
else
  printf '%s\n' "${RESULT}"
fi
