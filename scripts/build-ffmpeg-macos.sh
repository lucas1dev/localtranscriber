#!/usr/bin/env bash
# Compila o FFmpeg LGPL mínimo (arm64, estático, sem bibliotecas externas) a
# partir do tarball oficial fixado em scripts/sidecars.lock.json e o copia para
# src-tauri/binaries/ffmpeg-aarch64-apple-darwin.
#
# O build fica em src-tauri/target/ffmpeg-build/<versão>-<hash da configuração>;
# se o binário dessa versão e configuração já existir lá, a compilação é pulada.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK="$ROOT/scripts/sidecars.lock.json"
TRIPLE="aarch64-apple-darwin"
DEST="$ROOT/src-tauri/binaries/ffmpeg-$TRIPLE"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "build-ffmpeg-macos.sh só roda em macOS arm64." >&2
  exit 1
fi

lock_get() { plutil -extract "$1" raw -o - "$LOCK"; }

VERSION="$(lock_get ffmpeg.version)"
SOURCE_URL="$(lock_get "ffmpeg.$TRIPLE.source_url")"
SHA256="$(lock_get "ffmpeg.$TRIPLE.sha256")"
CONFIGURE="$(lock_get "ffmpeg.$TRIPLE.configure")"
read -r -a CONFIGURE_FLAGS <<<"$CONFIGURE"
CONFIG_ID="$(printf '%s' "$CONFIGURE" | shasum -a 256 | cut -c1-12)"

CACHE="$ROOT/src-tauri/target/ffmpeg-build/$VERSION-$CONFIG_ID"
TARBALL="$CACHE/$(basename "$SOURCE_URL")"
SRC="$CACHE/src"
BUILT="$CACHE/ffmpeg"

mkdir -p "$CACHE" "$(dirname "$DEST")"

if [ -x "$BUILT" ]; then
  echo "FFmpeg $VERSION já compilado em $BUILT"
else
  if [ ! -f "$TARBALL" ]; then
    echo "Baixando $SOURCE_URL..."
    curl --fail -L -sS -o "$TARBALL.part" "$SOURCE_URL"
    mv "$TARBALL.part" "$TARBALL"
  fi
  actual="$(shasum -a 256 "$TARBALL" | awk '{print $1}')"
  if [ "$actual" != "$SHA256" ]; then
    echo "SHA-256 inválido para $TARBALL: esperado $SHA256, obtido $actual" >&2
    rm -f "$TARBALL"
    exit 1
  fi

  rm -rf "$SRC"
  mkdir -p "$SRC"
  tar -xJf "$TARBALL" -C "$SRC" --strip-components=1

  echo "Configurando FFmpeg $VERSION: ${CONFIGURE_FLAGS[*]}"
  (
    cd "$SRC"
    ./configure "${CONFIGURE_FLAGS[@]}"
    make -j"$(sysctl -n hw.ncpu)" ffmpeg
  )
  cp "$SRC/ffmpeg" "$BUILT.part"
  mv "$BUILT.part" "$BUILT"
fi

cp "$BUILT" "$DEST.part"
chmod 755 "$DEST.part"
mv "$DEST.part" "$DEST"
echo "FFmpeg $VERSION -> $DEST"
