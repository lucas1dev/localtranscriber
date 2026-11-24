# Local Transcriber — App desktop (Mac e Windows) e redesenho de usabilidade

Data: 2026-11-24 · Status: aprovado pelo autor

## 1. Intenção

Hoje o Local Transcriber é um servidor FastAPI + página web que só roda no Mac, depende de Homebrew e obriga a rolar a transcrição inteira para chegar ao resumo. Configurar modelos e chaves de API é espalhado pela tela principal ou feito no Terminal (`./install.sh`).

Queremos um **aplicativo desktop para Mac e Windows** que:

- funcione sem o usuário instalar nada além do próprio app (whisper.cpp, FFmpeg e Codex CLI vêm dentro);
- tenha um fluxo principal **passo a passo** (Áudio → Transcrição → Resumo), sem rolagem longa;
- tenha uma **tela de Configurações** própria para baixar modelos e conectar contas de IA;
- permita **entrar com a conta ChatGPT** sem chave de API.

**Para quem:** agora, uso próprio do autor. O código será compartilhado com colegas da empresa, que podem gerar o app eles mesmos ou baixar o gerado pelo build automático. Uso amplo na empresa é provável depois.

## 2. Objetivos

1. App Tauri v2 para **macOS Apple Silicon** e **Windows x64**, gerado a partir do repositório.
2. Transcrição local com whisper.cpp **compilado dentro do app** (Metal no Mac, CPU no Windows).
3. FFmpeg e Codex CLI embutidos como executáveis auxiliares (sidecars).
4. Fluxo principal em 3 passos com navegação de volta sem perder estado.
5. Tela de Configurações com menu lateral: modelos Whisper (baixar/remover/padrão), contas de IA, Ollama.
6. "Entrar com ChatGPT" via Codex CLI embutido.
7. Resumos com OpenAI, Anthropic, Gemini (chave de API), ChatGPT (via Codex), Ollama (local), mantendo os formatos e o map-reduce atuais.
8. Resumo exibido em Markdown formatado.
9. Build automático (GitHub Actions) gerando os executáveis de Mac e Windows sem assinatura.

## 3. Não-objetivos (por agora)

- Assinatura de código, notarização Apple, instalador assinado, atualização automática. (Não devem ser bloqueados: usar o identificador e a estrutura de bundle padrão do Tauri para poder ligar depois.)
- Histórico persistente de transcrições/projetos (SQLite). O estado vive só durante a sessão do app.
- Embutir modelos Whisper (1,6 GB) ou o Ollama no instalador.
- Mac Intel, Linux.
- Modo navegador (`./start.sh`). Ele deixa de existir.
- Streaming palavra a palavra na gravação ao vivo (continua em blocos).

## 4. Anti-objetivos (seria fracasso mesmo funcionando)

- Pedir ao usuário senha, cookie ou token do ChatGPT/Claude, ou guardar credenciais fora do keychain do sistema / da pasta do próprio usuário.
- Redistribuir software que não permite redistribuição (Claude Code).
- Mandar áudio ou transcrição para a nuvem sem o usuário acionar um provedor de nuvem explicitamente.
- Exigir Terminal, Homebrew ou Python para **usar** o app.
- Tela principal voltar a acumular configuração (chaves, downloads) no meio do fluxo.

## 5. Restrições e fatos verificados

- **Claude Code** é proprietário e não pode ir dentro do app. A documentação legal da Anthropic diz que apps de terceiros não podem oferecer login claude.ai nem encaminhar pedidos pelas credenciais de plano dos usuários; produtos devem usar chave de API (code.claude.com/docs/en/legal-and-compliance). → O caminho principal do Claude é **por chave de API**. O uso do Claude Code já instalado por quem roda o app é uma decisão consciente do autor, com esse risco registrado (ver 7.4).
- **Codex CLI** é open source (Apache 2.0): pode ir embutido. O login com ChatGPT é feito pelo fluxo oficial do próprio Codex, os tokens ficam locais e sob controle do usuário, e as chamadas partem da máquina do usuário — compatível com os termos "Sign in with ChatGPT" da OpenAI (openai.com/policies/sign-in-with-chatgpt-terms). Uso conta nos limites do plano ChatGPT do usuário.
- **Credenciais atuais** estão no Keychain do Mac sob o serviço `local-transcriber-ai`, conta = nome do provedor. O crate `keyring` lê o mesmo item, então quem já salvou chave não perde.
- **FFmpeg**: usar build **LGPL** (não GPL) para redistribuição.
- **Compilar whisper.cpp no Windows** exige Visual Studio Build Tools, CMake e LLVM/clang. Por isso o build automático no GitHub é parte do escopo.
- O app aberto pelo Finder/menu Iniciar não herda o PATH do Terminal.

## 6. Abordagens consideradas

| | Abordagem | Trade-off | Escolha |
|---|---|---|---|
| Casca desktop | **Tauri v2** | App leve, nativo; exige Rust + Node para gerar; WebKit no Mac (microfone exige permissão) | **Escolhida pelo autor** |
| | pywebview + PyInstaller | Só Python para gerar; antivírus costuma bloquear PyInstaller no Windows | Descartada |
| | Electron + Python | Microfone funciona direto; app > 150 MB, Node + Python | Descartada |
| Backend | **Reescrever em Rust** (~220 linhas atuais) | Dois toolchains (Rust, Node); perde modo navegador | **Escolhida** |
| | Python como sidecar | Preserva código; três toolchains, PyInstaller + antivírus | Descartada |
| Frontend | **JS puro em módulos ES, sem bundler** | Sem build de frontend; estado gerenciado à mão | **Escolhida** (Node só para o Tauri CLI) |
| | Svelte/React + Vite | Melhor gestão de estado; mais peso para uma interface pequena | Descartada |
| Layout | **Passo a passo** | Guiado, sem rolagem longa; mais cliques para uso frequente | **Escolhido pelo autor** |
| | Lado a lado / Abas | — | Descartados |
| Configurações | **Página com menu lateral** | Espaço para listas e downloads | **Escolhida pelo autor** |
| | Painel deslizante | — | Descartado |
| Login de CLIs | **Detectar e orientar** (não instalar por conta própria) | — | **Escolhido pelo autor**; com Codex embutido, só o login precisa de ação |

## 7. Design

### 7.1 Estrutura do repositório

```
src-tauri/            app Rust (Tauri v2)
  src/
    main.rs           setup, registro de comandos
    transcribe.rs     ffmpeg → PCM 16 kHz mono → whisper-rs
    models.rs         catálogo, download com progresso, remover
    providers/        openai.rs, anthropic.rs, gemini.rs, codex.rs, ollama.rs, claude_code.rs
    summarize.rs      prompts, chunking, map-reduce, compare
    credentials.rs    keyring
    settings.rs       preferências em JSON
    paths.rs          pasta de dados, localizar sidecars e CLIs
  binaries/           sidecars por target triple (baixados por script, não versionados)
  tauri.conf.json
ui/                   frontend estático (frontendDist)
  index.html
  css/
  js/  app.js, state.js, router.js, steps/audio.js, steps/transcript.js, steps/summary.js, settings/*.js
  vendor/  marked.min.js, purify.min.js (versionados, para funcionar offline)
scripts/  fetch-sidecars.(sh|ps1)
.github/workflows/build.yml
```

`app.py`, `ai_engine.py`, `static/`, `install.sh`, `start.sh`, `requirements.txt` são **removidos** quando o port estiver completo. O README é reescrito.

### 7.2 Comunicação frontend ↔ backend

Comandos Tauri (`invoke`) no lugar dos endpoints HTTP, com `withGlobalTauri: true` (sem bundler). Progresso via eventos Tauri. Comandos principais:

- `get_status` → engine pronta, sidecars encontrados, modelos instalados
- `transcribe_file(path, model, language)` → `{text, segments[{start,end,text}]}`; emite `transcribe://progress`; `cancel_transcription()`
- `transcribe_chunk(bytes, mime, model, language)` → para o microfone
- `list_models` / `download_model(id)` (eventos `model://progress`) / `cancel_download(id)` / `delete_model(id)` / `set_default_model(id)`
- `providers_status` → por provedor: `configured`, `detail` (ex.: "logado como …", "não está rodando")
- `save_api_key(provider, key)` / `delete_api_key(provider)`
- `codex_login()` (abre navegador, emite resultado) / `codex_logout()`
- `ollama_models` / `ollama_pull(name)` (eventos) / `ollama_delete(name)`
- `summarize(req)` / `compare(req)`
- `get_settings` / `set_settings`

Arquivos de áudio escolhidos pelo seletor nativo (`dialog` plugin) ou arrastados na janela: o backend recebe o **caminho**, não os bytes.

### 7.3 Transcrição

- Crate `whisper-rs` com feature `metal` no macOS; Windows somente CPU nesta versão.
- FFmpeg sidecar converte qualquer entrada para PCM f32 16 kHz mono (via stdout, sem arquivo intermediário quando possível).
- Segmentos com timestamps no mesmo formato atual (`00:00:00,000`) para manter a exportação SRT.
- Execução em thread de trabalho; progresso via callback do whisper-rs; cancelável.
- Modelo carregado fica em cache em memória enquanto o mesmo modelo for usado (evita recarregar 1,6 GB a cada bloco do microfone).

**Microfone:** `getUserMedia` + `MediaRecorder` na webview em blocos de ~12 s (como hoje), cada bloco enviado a `transcribe_chunk`. No Mac, `NSMicrophoneUsageDescription` no Info.plist. **Spike obrigatório no início:** confirmar que o microfone funciona no WebKit do Tauri em macOS e na WebView2 do Windows. Se não funcionar no Mac, gravar nativamente no Rust com o crate `cpal` e manter a mesma interface.

### 7.4 Provedores de resumo

Prompts, formatos (executivo, detalhado, acadêmico, reunião, aula, podcast, personalizado), chunking (~22 mil caracteres) e map-reduce são **portados como estão** de `ai_engine.py`, assim como os modelos padrão por provedor.

| Provedor | Como | Configuração |
|---|---|---|
| ChatGPT (assinatura) | Codex CLI embutido: `codex exec` não interativo, só leitura, prompt via stdin, resposta final capturada | Botão "Entrar com ChatGPT" → `codex login` |
| OpenAI (API) | HTTP Responses API | Chave no keychain |
| Anthropic (API) | HTTP Messages API | Chave no keychain |
| Gemini (API) | HTTP generateContent | Chave no keychain |
| Ollama (local) | HTTP `127.0.0.1:11434` | Detecta; lista/baixa/remove modelos |
| Claude Code (opcional) | `claude -p` do Claude Code que o usuário já tenha instalado e logado | Desligado por padrão; ver nota |

- Codex usa um `CODEX_HOME` próprio dentro da pasta de dados do app, para não conflitar com uma instalação do Codex que o usuário já tenha.
- **Claude Code (decisão do autor):** presente em **todos os builds**, inclusive os do GitHub, **desligado por padrão**. Quando ligado, usa o Claude Code que **a própria pessoa que roda o app** instalou e logou no computador dela, procurado em `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, `%USERPROFILE%\.local\bin`, `%APPDATA%\npm`, além do PATH. O app nunca faz login no Claude, nunca lê nem guarda credenciais do Claude e nunca usa credenciais de quem compilou. Se não encontrar `claude` ou se ele não estiver logado (`claude auth status`), a opção mostra o estado e a orientação, sem tentar instalar. O autor está ciente de que a regra da Anthropic citada em 5 se aplica a apps de terceiros e aceitou esse risco; o texto na tela deixa claro que o uso conta no plano Claude da própria pessoa.
- **Comparar:** o usuário marca quais provedores comparar; só aparecem marcáveis os configurados/disponíveis (corrige o comportamento atual de chamar os quatro).

### 7.5 Tela principal — passo a passo

```
 ① Áudio ─── ② Transcrição ─── ③ Resumo                       ⚙
```

- **① Áudio:** abas Arquivo / Microfone; área de soltar arquivo + seletor nativo; modelo e idioma (valores padrão vêm das Configurações). Modelos não instalados aparecem desabilitados com link "Baixar em Configurações". Botão Transcrever. Progresso com porcentagem e Cancelar. Atalho "Já tenho o texto → colar" pula para o passo 2. No microfone, o texto aparece ao vivo; ao finalizar, avança para o passo 2.
- **② Transcrição:** cabeçalho com arquivo · modelo · idioma; texto editável com rolagem própria; segmentos com horário numa área recolhível; Copiar / TXT / SRT; "← Voltar" e "Resumir →".
- **③ Resumo:** provedor (só os configurados; os demais com link para Configurações), formato, instruções personalizadas, modelo opcional. Gerar / Comparar. Resultado em Markdown renderizado (marked + DOMPurify), com Copiar e Salvar .md. Na comparação, cartões lado a lado.
- Passos concluídos ficam clicáveis. Voltar não apaga nada; uma nova transcrição substitui a anterior após confirmação se o texto foi editado.
- Erros aparecem dentro da tela (faixa de erro no passo), não em `alert()`.
- Se nada estiver pronto na primeira abertura (sem modelo Whisper), o passo ① mostra um aviso com botão direto para Configurações → Transcrição.

### 7.6 Configurações — página com menu lateral

```
← Voltar · Configurações
┌───────────────┬─────────────────────────────────────────────┐
│ Transcrição   │ Modelos Whisper                             │
│ Contas de IA  │ Large V3 Turbo · 1,6 GB · ✓ · padrão ● [Remover]
│ Ollama        │ Medium · 1,5 GB · [Baixar]                  │
│ Sobre         │ Small · 466 MB · ▓▓▓░ 62% [Cancelar]        │
│               │ Idioma padrão ▾                             │
└───────────────┴─────────────────────────────────────────────┘
```

- **Transcrição:** catálogo base / small / medium / large-v3-turbo com tamanho, estado, Baixar (barra de progresso, Cancelar), Remover, Definir padrão. Download do Hugging Face (`ggerganov/whisper.cpp`) para `<dados do app>/models`, em arquivo `.part` renomeado só ao concluir. Idioma padrão.
- **Contas de IA:**
  - *Pela sua assinatura:* ChatGPT (Codex embutido) — Entrar / Sair / estado. Claude Code — conforme 7.4.
  - *Por chave de API:* OpenAI, Anthropic, Gemini — estado, Adicionar / Trocar / Remover. A chave nunca volta para a interface.
  - Aviso curto de que chaves de API geram cobrança por uso.
- **Ollama:** estado (rodando / não encontrado com link de download), modelos instalados, campo "baixar modelo" com progresso, remover.
- **Sobre:** versão, pasta de dados (com botão "Abrir pasta"), versões de whisper.cpp / FFmpeg / Codex embutidos.

### 7.7 Dados do app

- Identificador do app: **`io.github.lucas1dev.localtranscriber`** (nome do autor; fixo daqui em diante, pois define a pasta de dados, a permissão de microfone e uma futura assinatura). Nome exibido: "Local Transcriber".
- Pasta de dados via API do Tauri (`~/Library/Application Support/<identificador>` no Mac, `%APPDATA%\<identificador>` no Windows): `models/`, `codex-home/`, `settings.json`.
- `settings.json`: modelo padrão, idioma padrão, provedor/formato padrão de resumo, Claude Code ligado/desligado.
- Modelos existentes em `./models` do projeto antigo: o app oferece **importar** (mover/copiar) na primeira abertura se encontrar o arquivo no caminho antigo. *(Conveniência para o autor; pode ser só um botão em Configurações.)*

### 7.8 Build e distribuição

- `scripts/fetch-sidecars.sh` / `.ps1` baixam FFmpeg (LGPL) e Codex CLI na versão fixada para `src-tauri/binaries/<nome>-<target-triple>`; versões e checksums fixados no script.
- `npm run tauri build` gera `.app`/`.dmg` (Mac) e `.msi`/`.exe` (Windows).
- Repositório **público** na conta pessoal do autor no GitHub: `git@github.com:lucas1dev/localtranscriber.git`.
- GitHub Actions: jobs `macos-14` (arm64) e `windows-latest` (x64), sem assinatura. Em todo push, gera os instaladores como artefatos (para teste). Ao criar uma tag `v*`, publica os instaladores numa **GitHub Release**, que é o link estável para colegas baixarem.
- README: como baixar da Release, como passar pelo aviso do Gatekeeper / SmartScreen por não ser assinado, e como gerar localmente.
- O projeto ainda não é um repositório git; criar o repositório faz parte do trabalho (sem segredos: `.superpowers/`, `models/`, `outputs/`, `src-tauri/binaries/` e `target/` no `.gitignore`).

### 7.9 Ordem sugerida de construção

1. Spike: Tauri mínimo + microfone no Mac e no Windows + whisper-rs transcrevendo um arquivo com Metal.
2. Backend Rust: transcrição, modelos, credenciais, provedores, resumo (port do Python).
3. Frontend: passo a passo e Configurações.
4. Sidecars + build local + GitHub Actions.
5. Remover o código Python e reescrever o README.

## 8. Deixado para quem constrói

- Visual fino (cores, tipografia), mantendo a identidade escura atual com o verde `#b7f36b`.
- Nome e flags exatos de `codex exec`/`codex login` na versão fixada.
- Biblioteca HTTP (reqwest), estrutura de erros e de testes no Rust.
- Retomada de download interrompido (desejável, não obrigatória).
- Versões exatas de whisper.cpp/whisper-rs, FFmpeg e Codex, desde que fixadas.
