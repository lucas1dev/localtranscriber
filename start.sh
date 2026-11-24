#!/bin/bash
set -e
cd "$(dirname "$0")"
if [ ! -d .venv ]; then echo "Execute primeiro ./install.sh large-v3-turbo"; exit 1; fi
.venv/bin/uvicorn app:app --host 127.0.0.1 --port 8765
