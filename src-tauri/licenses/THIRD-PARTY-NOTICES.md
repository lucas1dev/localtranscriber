# Avisos de terceiros / Third-party notices

O Local Transcriber (https://github.com/lucas1dev/localtranscriber) inclui os
componentes abaixo. Os textos completos das licenças estão nesta pasta:
`LGPL-2.1.txt`, `LGPL-3.0.txt`, `GPL-3.0.txt` (incorporada pela LGPL-3.0) e
`Apache-2.0.txt`. Os avisos MIT estão no fim deste arquivo.

| Componente | Versão | Licença | Código-fonte |
|---|---|---|---|
| whisper.cpp (via whisper-rs 0.16.0) | 1.8.3 | MIT | https://github.com/ggml-org/whisper.cpp |
| FFmpeg (macOS, Apple Silicon) | 9.1 | LGPL-2.1-or-later | https://ffmpeg.org/releases/ffmpeg-9.1.tar.xz |
| FFmpeg (Windows x64) | n9.0.2-39-g7adfbe40dc | LGPL-3.0-or-later | https://github.com/FFmpeg/FFmpeg/archive/7adfbe40dcde598f93a6963d2f7b22cd75202ccc.tar.gz |
| Codex CLI | 0.172.0 | Apache-2.0 | https://github.com/openai/codex/tree/rust-v0.172.0 |
| marked | 18.2.1 | MIT | https://github.com/markedjs/marked |
| DOMPurify | 3.4.18 | Apache-2.0 OR MPL-2.0 (usado sob Apache-2.0) | https://github.com/cure53/DOMPurify |

## FFmpeg

O FFmpeg roda como executável separado (sidecar) e não é vinculado ao app.

- **macOS:** FFmpeg 9.1 compilado do código-fonte oficial, em uma configuração
  mínima (`--disable-gpl --disable-nonfree`, sem `--enable-version3`), sob
  LGPL 2.1 ou posterior. Código-fonte:
  https://ffmpeg.org/releases/ffmpeg-9.1.tar.xz. O script e as flags exatas
  estão em `scripts/build-ffmpeg-macos.sh` e `scripts/sidecars.lock.json` no
  repositório (https://github.com/lucas1dev/localtranscriber).
- **Windows:** build `win64-lgpl-9.0` do projeto BtbN/FFmpeg-Builds, release
  https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-10-31-12-59
  (scripts de build no commit 5031c6f1082414e53688d39e521849905071070d), sob
  LGPL 3 ou posterior (o build sempre usa `--enable-version3`; nunca
  `--enable-gpl` nem `--enable-nonfree`). Código-fonte do FFmpeg na revisão
  7adfbe40dcde598f93a6963d2f7b22cd75202ccc, linkado na tabela acima.

Oferta de código-fonte: o código-fonte correspondente dos binários do FFmpeg
distribuídos com este app está nos links acima; se algum deles deixar de
funcionar, abra uma issue em https://github.com/lucas1dev/localtranscriber/issues
e o código será fornecido. Você pode substituir o FFmpeg embutido por uma
versão sua modificada.

## Avisos MIT

### whisper.cpp

MIT License. Copyright (c) 2023-2024 The ggml authors

### marked

MIT License. Copyright (c) 2018-2026, MarkedJS. Copyright (c) 2011-2018,
Christopher Jeffrey.

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

(O mesmo texto de permissão MIT acima se aplica ao whisper.cpp.)
