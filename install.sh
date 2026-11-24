#!/bin/bash
set -e
cd "$(dirname "$0")"
MODEL="${1:-large-v3-turbo}"
case "$MODEL" in base|small|medium|large-v3-turbo) ;; *) echo "Modelo válido: base, small, medium, large-v3-turbo"; exit 1;; esac
if ! command -v brew >/dev/null 2>&1; then echo "Instale Homebrew: https://brew.sh"; exit 1; fi
brew install whisper.cpp ffmpeg python@3.12
PYTHON="$(brew --prefix python@3.12)/bin/python3.12"
if [ ! -x "$PYTHON" ]; then echo "Python 3.12 não encontrado em $PYTHON"; exit 1; fi
if [ -x .venv/bin/python ] && ! .venv/bin/python -c 'import sys; sys.exit(sys.version_info < (3, 12))'; then
  echo "Recriando .venv com Python 3.12..."
  rm -rf .venv
fi
"$PYTHON" -m venv .venv
.venv/bin/pip install -r requirements.txt
mkdir -p models
MODEL_FILE="$PWD/models/ggml-${MODEL}.bin"
if [ ! -s "$MODEL_FILE" ]; then
  echo "Baixando modelo $MODEL..."
  curl -L --fail --progress-bar "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-${MODEL}.bin" -o "$MODEL_FILE"
fi
echo "Instalação concluída. Modelo: $MODEL"
echo "Execute ./start.sh para iniciar."
