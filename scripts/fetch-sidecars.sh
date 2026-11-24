#!/usr/bin/env bash
# Prepara os sidecars do macOS (aarch64-apple-darwin) em src-tauri/binaries:
#   - codex-aarch64-apple-darwin: baixado da GitHub Release fixada no lock;
#   - ffmpeg-aarch64-apple-darwin: compilado por build-ffmpeg-macos.sh.
# Aborta com código != 0 se algum SHA-256 não bater com o lock.
# No Windows, use scripts/fetch-sidecars.ps1.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK="$ROOT/scripts/sidecars.lock.json"
TRIPLE="aarch64-apple-darwin"
BIN="$ROOT/src-tauri/binaries"
CACHE="$ROOT/src-tauri/target/sidecars"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "fetch-sidecars.sh só roda em macOS arm64; no Windows use fetch-sidecars.ps1." >&2
  exit 1
fi

lock_get() { plutil -extract "$1" raw -o - "$LOCK"; }

# Baixa $1 para $2 (reaproveitando o cache) e confere o SHA-256 $3.
download_verified() {
  local url="$1" file="$2" expected="$3" actual
  if [ ! -f "$file" ]; then
    echo "Baixando $url..."
    curl --fail -L -sS -o "$file.part" "$url"
    mv "$file.part" "$file"
  fi
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  if [ "$actual" != "$expected" ]; then
    echo "SHA-256 inválido para $file: esperado $expected, obtido $actual" >&2
    rm -f "$file"
    exit 1
  fi
}

mkdir -p "$BIN" "$CACHE"

# --- Codex ------------------------------------------------------------------
CODEX_VERSION="$(lock_get codex.version)"
CODEX_URL="$(lock_get "codex.$TRIPLE.url")"
CODEX_SHA256="$(lock_get "codex.$TRIPLE.sha256")"
CODEX_MEMBER="$(lock_get "codex.$TRIPLE.member")"
CODEX_ARCHIVE="$CACHE/codex-$CODEX_VERSION-$(basename "$CODEX_URL")"

download_verified "$CODEX_URL" "$CODEX_ARCHIVE" "$CODEX_SHA256"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tar -xzf "$CODEX_ARCHIVE" -C "$work" "$CODEX_MEMBER"
install -m 755 "$work/$CODEX_MEMBER" "$BIN/codex-$TRIPLE"
echo "Codex $CODEX_VERSION -> $BIN/codex-$TRIPLE"

# --- FFmpeg -----------------------------------------------------------------
"$ROOT/scripts/build-ffmpeg-macos.sh"
