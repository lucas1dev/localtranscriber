# Fixtures de teste

Os testes ignorados de `src-tauri/tests/transcribe_real.rs` usam `jfk.wav`, a amostra
oficial do whisper.cpp (11 s, 16 kHz, mono). O arquivo não é versionado.

Para obtê-lo, rode na raiz do repositório:

```sh
scripts/fetch-test-fixtures.sh
```

O script baixa `https://github.com/ggerganov/whisper.cpp/raw/master/samples/jfk.wav`
para esta pasta (só se ainda não existir) e confere o SHA-256.

Depois, dentro de `src-tauri/`:

```sh
LT_FFMPEG=$(which ffmpeg) LT_MODEL=/caminho/para/ggml-large-v3-turbo.bin \
  cargo test --test transcribe_real -- --ignored --nocapture
```
