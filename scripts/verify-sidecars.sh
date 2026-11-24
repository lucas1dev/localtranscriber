#!/usr/bin/env bash
# Confere os sidecars do macOS em src-tauri/binaries:
#   - ffmpeg e codex presentes e executáveis;
#   - ffmpeg na versão do lock, sem --enable-gpl/--enable-nonfree e sem
#     bibliotecas fora do sistema;
#   - codex --version igual à versão do lock;
#   - o ffmpeg empacotado decodifica os formatos aceitos pelo app (mp3, m4a,
#     webm, ogg, flac, wav e mp4 com vídeo), com os mesmos argumentos de
#     transcribe::ffmpeg_args. A mídia de teste é gerada com o ffmpeg do
#     PATH (ex.: Homebrew) ou de HOST_FFMPEG=<caminho>; sem ele, a verificação
#     falha. Para pular de propósito essa parte, use SKIP_DECODE=1.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK="$ROOT/scripts/sidecars.lock.json"
BIN="$ROOT/src-tauri/binaries"
TRIPLE="aarch64-apple-darwin"
FFMPEG="$BIN/ffmpeg-$TRIPLE"
CODEX="$BIN/codex-$TRIPLE"

failures=0
fail() { echo "FALHA: $*" >&2; failures=$((failures + 1)); }
ok() { echo "ok: $*"; }

lock_get() { plutil -extract "$1" raw -o - "$LOCK"; }

[ -f "$LOCK" ] || { echo "FALHA: lock ausente: $LOCK" >&2; exit 1; }

for exe in "$FFMPEG" "$CODEX"; do
  if [ -x "$exe" ]; then ok "presente: $(basename "$exe")"; else fail "sidecar ausente: $exe"; fi
done
if [ "$failures" -ne 0 ]; then
  echo "$failures verificação(ões) falharam." >&2
  exit 1
fi

# --- FFmpeg: versão e licença ---------------------------------------------
ffmpeg_version_out="$("$FFMPEG" -hide_banner -version)"
ffmpeg_lock_version="$(lock_get ffmpeg.version)"
first_line="$(printf '%s\n' "$ffmpeg_version_out" | head -n 1)"
if [[ "$first_line" == "ffmpeg version $ffmpeg_lock_version "* ]]; then
  ok "ffmpeg $ffmpeg_lock_version"
else
  fail "versão do ffmpeg: esperado $ffmpeg_lock_version, obtido: $first_line"
fi
ffmpeg_lock_configure="$(lock_get ffmpeg.aarch64-apple-darwin.configure)"
# O ffmpeg cita entre aspas simples os valores com vírgula; compara sem elas.
ffmpeg_configure="$(printf '%s\n' "$ffmpeg_version_out" | sed -n "s/^configuration: //p" | tr -d "'")"
if [ "$ffmpeg_configure" = "$ffmpeg_lock_configure" ]; then
  ok "ffmpeg compilado com a configuração do lock"
else
  fail "a configuração do ffmpeg difere do lock"
fi
if printf '%s' "$ffmpeg_version_out" | grep -Eq 'enable-(gpl|nonfree|version3)'; then
  fail "ffmpeg foi configurado com --enable-gpl/--enable-nonfree/--enable-version3"
else
  ok "ffmpeg sem --enable-gpl"
fi
foreign_libs="$(otool -L "$FFMPEG" | tail -n +2 | awk '{print $1}' | grep -Ev '^(/usr/lib/|/System/Library/)' || true)"
if [ -n "$foreign_libs" ]; then
  fail "ffmpeg depende de bibliotecas fora do sistema: $foreign_libs"
else
  ok "ffmpeg só usa bibliotecas do sistema"
fi

# --- Codex: versão ----------------------------------------------------------
codex_lock_version="$(lock_get codex.version)"
codex_home="$(mktemp -d)"
codex_out="$(CODEX_HOME="$codex_home" "$CODEX" --version 2>&1 || true)"
rm -rf "$codex_home"
if [ "$codex_out" = "codex-cli $codex_lock_version" ]; then
  ok "$codex_out"
else
  fail "versão do codex: esperado codex-cli $codex_lock_version, obtido: $codex_out"
fi

# --- FFmpeg: decodificação dos formatos aceitos -----------------------------
# (src-tauri/binaries não está no PATH, então `command -v` acha o do sistema.)
HOST_FFMPEG="${HOST_FFMPEG:-$(command -v ffmpeg || true)}"

if [ "${SKIP_DECODE:-}" = "1" ]; then
  echo "AVISO: SKIP_DECODE=1 — o teste de decodificação NÃO foi executado; os formatos aceitos pelo app não foram verificados." >&2
elif [ -z "$HOST_FFMPEG" ]; then
  fail "ffmpeg do sistema não encontrado para gerar a mídia de teste (instale com 'brew install ffmpeg', defina HOST_FFMPEG=<caminho> ou pule com SKIP_DECODE=1)"
else
  media="$(mktemp -d)"
  trap 'rm -rf "$media"' EXIT
  gen() { "$HOST_FFMPEG" -nostdin -hide_banner -loglevel error -y "$@"; }
  tone=(-f lavfi -i "sine=frequency=440:duration=2")
  gen "${tone[@]}" -c:a libmp3lame "$media/audio.mp3"
  gen "${tone[@]}" -c:a aac "$media/audio.m4a"
  gen "${tone[@]}" -c:a libopus "$media/audio.webm"
  gen "${tone[@]}" -c:a vorbis -strict experimental -ac 2 "$media/audio.ogg"
  gen "${tone[@]}" -c:a flac "$media/audio.flac"
  gen "${tone[@]}" -c:a pcm_s16le "$media/audio.wav"
  gen -f lavfi -i "testsrc=size=160x120:rate=10:duration=2" "${tone[@]}" \
    -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest "$media/Vídeo com áudio.mp4"

  for input in "$media"/audio.* "$media/Vídeo com áudio.mp4"; do
    name="$(basename "$input")"
    # Mesmos argumentos de transcribe::ffmpeg_args.
    if "$FFMPEG" -nostdin -hide_banner -loglevel error -i "$input" \
        -ac 1 -ar 16000 -f f32le - >"$media/out.pcm" 2>"$media/err.txt" \
        && [ -s "$media/out.pcm" ]; then
      ok "decodifica $name ($(wc -c <"$media/out.pcm" | tr -d ' ') bytes)"
    else
      fail "não decodificou $name: $(cat "$media/err.txt")"
    fi
  done
fi

if [ "$failures" -ne 0 ]; then
  echo "$failures verificação(ões) falharam." >&2
  exit 1
fi
echo "Sidecars OK."
