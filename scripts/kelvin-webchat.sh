#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEB_DIR="${ROOT_DIR}/apps/kelvin-gateway/web"
PORT="${1:-3180}"

if [[ ! -f "${WEB_DIR}/index.html" ]]; then
  echo "[kelvin-webchat] missing ${WEB_DIR}/index.html" >&2
  exit 1
fi

if command -v python3 >/dev/null 2>&1; then
  echo "[kelvin-webchat] serving ${WEB_DIR} at http://127.0.0.1:${PORT}"
  cd "${WEB_DIR}"
  exec python3 -m http.server "${PORT}"
fi

echo "[kelvin-webchat] python3 is required to serve web UI" >&2
exit 1
