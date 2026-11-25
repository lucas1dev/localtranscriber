# Local Transcriber

Aplicativo de desktop (macOS Apple Silicon e Windows x64) que transcreve áudio no seu próprio computador, usando Whisper (whisper.cpp), e gera resumos com a IA que você escolher. Você pode importar arquivos de áudio ou gravar o microfone e o som do computador (reuniões, vídeos), e exportar o resultado.

## Baixar e instalar

Baixe o instalador da versão mais recente em [Releases](https://github.com/lucas1dev/localtranscriber/releases): `.dmg` para Mac ou instalador `.msi`/`.exe` para Windows. No Mac, é preciso macOS 14.6 ou mais novo.

O app ainda não é assinado digitalmente, então o sistema avisa na primeira abertura.

**macOS (Gatekeeper):** arraste o app para Aplicativos, clique com o botão direito nele e escolha **Abrir**, depois confirme em **Abrir**. Se o macOS bloquear mesmo assim, abra **Ajustes do Sistema → Privacidade e Segurança**, role até a mensagem sobre o app e clique em **Abrir mesmo assim**.

Se nada disso aparecer e o macOS disser que o app "está danificado e não pode ser aberto" (acontece com apps baixados pelo navegador que não têm assinatura da Apple), tire a marca de quarentena no Terminal e abra de novo. Use só com o app baixado da página de Releases deste repositório:

```sh
xattr -dr com.apple.quarantine "/Applications/Local Transcriber.app"
```

**Windows (SmartScreen):** na tela "O Windows protegeu seu computador", clique em **Mais informações** e depois em **Executar assim mesmo**.

## Primeiro uso

1. Abra **Configurações → Transcrição** e baixe um modelo Whisper (os maiores são mais precisos e mais lentos).
2. Volte para a tela inicial, importe um áudio ou grave (microfone e/ou som do computador) e transcreva.
3. Exporte a transcrição em TXT ou SRT, ou o resumo em `.md`: o app pergunta onde salvar o arquivo.

A transcrição roda 100% no seu computador, sem internet depois que o modelo é baixado.

## Gravar

Na aba **Gravar** do passo ①, escolha a fonte:

- **Microfone:** grava o microfone escolhido na lista (o padrão do sistema vem marcado; o botão ↻ recarrega a lista).
- **Som do computador:** grava o que está tocando no computador (reunião, vídeo, aula). Com **Saída de som** você escolhe qual saída gravar (por exemplo, o fone da reunião); se a escolhida for desconectada no meio, a gravação continua na primeira opção (todas as saídas no macOS, a saída padrão no Windows) e o app avisa.
- **Microfone + som do computador:** grava os dois ao mesmo tempo. No texto, cada fala vem etiquetada: **Você** (microfone) e **Reunião** (som do computador). As etiquetas também aparecem nas legendas SRT e no texto enviado para o resumo.

Enquanto grava, uma bolinha vermelha pulsa ao lado do cronômetro e uma barra de nível por fonte mostra se há som entrando. Barra parada em "Reunião" significa que nada está tocando no computador ou, no macOS, que falta a permissão para gravar o som do computador; parada em "Você", que o microfone não está captando ou não tem permissão.

**Dica:** use fone de ouvido. Sem fone, o microfone capta a fala dos outros saindo pelo alto-falante e ela é gravada duas vezes.

**macOS:** na primeira vez que gravar, o macOS pede a permissão **Microfone** (para o microfone) e **Gravação de áudio do sistema** (para o som do computador). Se você negar, ou quiser mudar depois, vá em **Ajustes do Sistema → Privacidade e Segurança**. Com o microfone negado, o app avisa antes de começar. Com a gravação do som do computador negada, o macOS pode entregar silêncio em vez de um erro: a barra "Reunião" fica parada e o app sugere conferir a permissão. Isso exige macOS 14.6 ou mais novo.

**Windows:** não há pedido de permissão, mas o acesso ao microfone precisa estar liberado em **Configurações → Privacidade e segurança → Microfone**, inclusive a opção para aplicativos da área de trabalho. Se estiver bloqueado, o app avisa ao começar a gravar.

**Fones Bluetooth:** quando o microfone do fone está em uso, o Bluetooth costuma passar para a qualidade de chamada e o som piora. Para uma qualidade melhor, grave com o microfone do computador e use o fone só para ouvir.

## Contas de IA para resumos

Resumir é opcional. Configure em **Configurações**:

- **ChatGPT:** entre com a sua conta pelo login embutido (usa o Codex CLI que acompanha o app). No Windows, o **Entrar com ChatGPT** ainda não foi verificado.
- **Chaves de API (OpenAI, Anthropic, Gemini):** cole a sua chave. **Atenção: o uso da API é cobrado pelo provedor, na sua conta.**
- **Claude Code (opcional):** se você já tem o Claude Code instalado e logado, o app pode usá-lo. **Ele usa o plano da própria pessoa** (os limites e a cobrança são os da sua assinatura).
- **Ollama:** resumos locais, sem enviar nada para a nuvem. Instale o [Ollama](https://ollama.com/download) e baixe um modelo pelo app.

## Privacidade

O áudio e a transcrição ficam no seu computador. **Só sai da máquina o texto que você mandar resumir em um provedor de nuvem** (ChatGPT, OpenAI, Anthropic, Gemini ou Claude Code). Com o Ollama, nada sai do computador.

## Gerar localmente

Pré-requisitos:

- **macOS:** Xcode Command Line Tools (`xcode-select --install`), Rust, Node.js, CMake.
- **Windows:** Visual Studio Build Tools (carga de trabalho C++), Rust, Node.js, CMake, LLVM.

Passos (na raiz do repositório). Os sidecars (FFmpeg e Codex) precisam estar em `src-tauri/binaries/` **antes** de qualquer comando `cargo` ou `tauri`:

```bash
npm ci
scripts/fetch-sidecars.sh          # macOS; no Windows: powershell -ExecutionPolicy Bypass -File scripts\fetch-sidecars.ps1
npm run tauri build
```

No macOS o FFmpeg é compilado do código-fonte na primeira vez (fica em cache). Detalhes em [`scripts/README-sidecars.md`](scripts/README-sidecars.md). Testes: `cd src-tauri && cargo test` e `npm run test:ui`.

## Licenças de terceiros

O texto completo das licenças e os avisos acompanham o app (pasta `licenses` dentro do pacote; fontes em [`src-tauri/licenses/`](src-tauri/licenses/)) e aparecem em **Configurações → Sobre**.

| Componente | Licença |
|---|---|
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) 1.8.3 | MIT |
| FFmpeg | LGPL (veja abaixo) |
| [Codex CLI](https://github.com/openai/codex) 0.172.0 | Apache-2.0 |
| [marked](https://github.com/markedjs/marked) 18.2.1 | MIT |
| [DOMPurify](https://github.com/cure53/DOMPurify) 3.4.18 | Apache-2.0 ou MPL-2.0 |

### FFmpeg

O FFmpeg é distribuído como executável separado (não é vinculado ao app) e **nunca** com `--enable-gpl` ou `--enable-nonfree`.

- **macOS:** FFmpeg 9.1 compilado do código-fonte oficial em uma configuração mínima, sob **LGPL 2.1 ou posterior** (`LGPL-2.1-or-later`). Código-fonte desta versão: <https://ffmpeg.org/releases/ffmpeg-9.1.tar.xz>. Script de build, com as flags exatas: [`scripts/build-ffmpeg-macos.sh`](scripts/build-ffmpeg-macos.sh).
- **Windows:** build `win64-lgpl` do [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), FFmpeg n9.0.2-39-g7adfbe40dc, sob **LGPL 3.0 ou posterior** (`LGPL-3.0-or-later`; esse build sempre usa `--enable-version3`). Release usada: <https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-10-31-12-59>. Código-fonte do FFmpeg nessa revisão: <https://github.com/FFmpeg/FFmpeg/archive/7adfbe40dcde598f93a6963d2f7b22cd75202ccc.tar.gz>.

**Oferta de código-fonte:** o código-fonte correspondente aos binários do FFmpeg distribuídos neste projeto está nos links acima e em [`scripts/`](scripts/) (versões e SHA-256 em [`scripts/sidecars.lock.json`](scripts/sidecars.lock.json)). Se algum link deixar de funcionar, abra uma issue em <https://github.com/lucas1dev/localtranscriber/issues> e o código será fornecido. Você pode trocar o FFmpeg embutido por uma versão modificada sua.
