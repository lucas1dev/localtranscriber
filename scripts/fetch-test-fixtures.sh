#!/usr/bin/env bash
# Baixa os arquivos de áudio usados pelos testes ignorados de src-tauri/tests.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURES="$ROOT/src-tauri/tests/fixtures"
JFK_URL="https://github.com/ggerganov/whisper.cpp/raw/master/samples/jfk.wav"
JFK_SHA256="59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e"
JFK="$FIXTURES/jfk.wav"

sha256() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    sha256sum "$1" | awk '{print $1}'
  fi
}

mkdir -p "$FIXTURES"
if [ ! -f "$JFK" ]; then
  echo "Baixando jfk.wav..."
  curl --fail -L -sS -o "$JFK.part" "$JFK_URL"
  mv "$JFK.part" "$JFK"
fi

actual="$(sha256 "$JFK")"
if [ "$actual" != "$JFK_SHA256" ]; then
  echo "SHA-256 inválido para $JFK: esperado $JFK_SHA256, obtido $actual" >&2
  rm -f "$JFK"
  exit 1
fi
echo "jfk.wav OK ($JFK)"
