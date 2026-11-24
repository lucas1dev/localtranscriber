# Sidecars: FFmpeg (LGPL) e Codex

O app desktop embute dois programas externos ao lado do executável
(`bundle.externalBin` em `src-tauri/tauri.conf.json`):

| Sidecar | Arquivo em `src-tauri/binaries/` | Origem |
|---|---|---|
| FFmpeg | `ffmpeg-<triple>[.exe]` | macOS: compilado do código-fonte oficial; Windows: build LGPL do BtbN |
| Codex  | `codex-<triple>[.exe]`  | GitHub Release `openai/codex` |

Versões, URLs e SHA-256 ficam fixados em [`sidecars.lock.json`](sidecars.lock.json).
Os scripts abortam com código ≠ 0 se um SHA-256 não bater.

## Como preparar

Rode **antes** de `cargo build`, `cargo test` ou `npm run tauri build` (o
`tauri-build` exige os sidecars; sem eles falha com
``resource path `binaries/codex-...` doesn't exist``):

- macOS (Apple Silicon): `scripts/fetch-sidecars.sh` e depois `scripts/verify-sidecars.sh`.
  O FFmpeg é compilado uma vez e fica em cache em `src-tauri/target/ffmpeg-build/`.
  O `verify-sidecars.sh` usa o ffmpeg do sistema (Homebrew ou `HOST_FFMPEG=<caminho>`)
  para gerar mídia de teste; para pular essa parte de propósito, `SKIP_DECODE=1`.
- Windows: `powershell -ExecutionPolicy Bypass -File scripts\fetch-sidecars.ps1` e depois
  `powershell -ExecutionPolicy Bypass -File scripts\verify-sidecars.ps1` (versões do lock;
  falha se o `ffmpeg.exe -version` mostrar `--enable-gpl` ou `--enable-nonfree`).

## FFmpeg — licença LGPL

Nenhum dos binários é configurado com `--enable-gpl` ou `--enable-nonfree`.
A licença de cada alvo fica em `ffmpeg.<triple>.license` no lock:

- macOS: **LGPL 2.1 ou posterior** (`LGPL-2.1-or-later`; sem `--enable-version3`).
- Windows: **LGPL 3.0 ou posterior** (`LGPL-3.0-or-later`). O build `win64-lgpl` do
  BtbN sempre usa `--enable-version3` (`variants/defaults-lgpl.sh`), necessário para
  bibliotecas Apache-2.0 como a opencore-amr. O FFmpeg roda como executável separado,
  então a LGPL v3 não impõe nada ao código do app além de distribuir a fonte do FFmpeg.

### macOS (aarch64-apple-darwin) — FFmpeg 9.1 (LGPL-2.1-or-later)

- Código-fonte: <https://ffmpeg.org/releases/ffmpeg-9.1.tar.xz>
  (SHA-256 `1a4d394ae63d190a526cd052eb4415c156b5c5e2df37efab41a7de0b8fde2433`).
- Compilado por [`build-ffmpeg-macos.sh`](build-ffmpeg-macos.sh), estático e sem
  bibliotecas externas, com exatamente esta configuração (também em
  `ffmpeg.aarch64-apple-darwin.configure` no lock e visível em `ffmpeg -version`):

```
./configure --arch=arm64 --target-os=darwin --enable-static --disable-shared \
  --disable-gpl --disable-nonfree --disable-autodetect --enable-pthreads \
  --disable-everything --disable-network --disable-doc --disable-ffplay \
  --disable-ffprobe --enable-ffmpeg \
  --enable-demuxer=mov,mp3,wav,ogg,matroska,webm_dash_manifest,flac,aac \
  --enable-decoder=aac,mp3,mp3float,opus,vorbis,flac,pcm_s16le,pcm_s24le,pcm_f32le,alac \
  --enable-parser=aac,mpegaudio,opus,vorbis,flac \
  --enable-protocol=file,pipe --enable-filter=aresample,aformat,anull \
  --enable-encoder=pcm_f32le --enable-muxer=pcm_f32le --enable-swresample \
  --extra-cflags=-mmacosx-version-min=11.0 --extra-ldflags=-mmacosx-version-min=11.0
```

Para reproduzir: `scripts/build-ffmpeg-macos.sh` (baixa o tarball, confere o
SHA-256, configura e compila).

### Windows (x86_64-pc-windows-msvc) — FFmpeg n9.0.2-39-g7adfbe40dc (LGPL-3.0-or-later)

- Binário: build **win64-lgpl-9.0** (estático) do projeto
  [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), release
  [`autobuild-2026-10-31-12-59`](https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-10-31-12-59),
  arquivo `ffmpeg-n9.0.2-39-g7adfbe40dc-win64-lgpl-9.0.zip`
  (SHA-256 `9d37c564e0d01c18cde184a079fa6ef04d7d2783ada41da81d04165cc8f1a650`).
  Só o `bin/ffmpeg.exe` é embutido.
- Código-fonte do FFmpeg: revisão `7adfbe40dcde598f93a6963d2f7b22cd75202ccc`
  (branch `release/9.0`, n9.0.2 + 39 commits) —
  <https://github.com/FFmpeg/FFmpeg/archive/7adfbe40dcde598f93a6963d2f7b22cd75202ccc.tar.gz>.
- Scripts de build e configuração: BtbN/FFmpeg-Builds no commit
  [`5031c6f1082414e53688d39e521849905071070d`](https://github.com/BtbN/FFmpeg-Builds/tree/5031c6f1082414e53688d39e521849905071070d)
  (o commit da tag acima). As flags do `configure` são montadas por
  `variants/win64-lgpl.sh` + `variants/defaults-lgpl.sh` e pelas bibliotecas
  em `scripts.d/`; a linha completa usada aparece em
  `ffmpeg.exe -version` (linha `configuration:`). O `LICENSE.txt` do zip lista
  as licenças das bibliotecas incluídas.

## Codex 0.172.0

Binários oficiais da release
[`rust-v0.172.0`](https://github.com/openai/codex/releases/tag/rust-v0.172.0)
de `openai/codex` (licença Apache-2.0):

- macOS: `codex-aarch64-apple-darwin.tar.gz` (SHA-256 `bd121f8abd6b28b3fc476da04a348e25a83e89eda19d39107f68f8e966053092`)
- Windows: `codex-x86_64-pc-windows-msvc.exe.zip` (SHA-256 `c4292fc0414fc78682e733ff570ddec377922ebe1a29925a6d5bcc5aeff43ea0`), do qual só `codex-x86_64-pc-windows-msvc.exe` é embutido.
