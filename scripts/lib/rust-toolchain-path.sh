#!/usr/bin/env bash
set -euo pipefail

ensure_rust_toolchain_path() {
  if command -v cargo >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
    return 0
  fi

  if [[ -n "${HOME:-}" && -d "${HOME}/.rustup/toolchains" ]]; then
    local candidate=""
    for candidate in \
      "${HOME}/.rustup/toolchains/stable-aarch64-apple-darwin/bin" \
      "${HOME}/.rustup/toolchains/stable-x86_64-apple-darwin/bin" \
      "${HOME}/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin" \
      "${HOME}/.rustup/toolchains/stable-aarch64-unknown-linux-gnu/bin"
    do
      if [[ -x "${candidate}/cargo" ]]; then
        export PATH="${candidate}:${PATH}"
        break
      fi
    done
    if command -v cargo >/dev/null 2>&1; then
      return 0
    fi
    while IFS= read -r candidate; do
      candidate="$(dirname "${candidate}")"
      export PATH="${candidate}:${PATH}"
      if command -v cargo >/dev/null 2>&1; then
        return 0
      fi
    done < <(find "${HOME}/.rustup/toolchains" -maxdepth 3 -type f -name cargo 2>/dev/null)
  fi

  if [[ -n "${HOME:-}" && -d "${HOME}/.cargo/bin" ]]; then
    export PATH="${HOME}/.cargo/bin:${PATH}"
  fi
  if command -v cargo >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
    return 0
  fi

  if [[ -d "/usr/local/cargo/bin" ]]; then
    export PATH="/usr/local/cargo/bin:${PATH}"
  fi
  if command -v cargo >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
    return 0
  fi

  # Homebrew rustup installs (macOS arm64/intel)
  if [[ -d "/opt/homebrew/opt/rustup/bin" ]]; then
    export PATH="/opt/homebrew/opt/rustup/bin:${PATH}"
  fi
  if command -v cargo >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
    return 0
  fi
  if [[ -d "/usr/local/opt/rustup/bin" ]]; then
    export PATH="/usr/local/opt/rustup/bin:${PATH}"
  fi
  if command -v cargo >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
    return 0
  fi

  return 1
}
