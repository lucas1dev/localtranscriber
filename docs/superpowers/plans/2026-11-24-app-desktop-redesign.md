# Local Transcriber Desktop — Plano de Implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transformar o Local Transcriber (FastAPI + página web, só Mac) num app Tauri v2 para macOS Apple Silicon e Windows x64, com whisper.cpp, FFmpeg e Codex CLI embutidos, fluxo passo a passo e tela de Configurações.

**Architecture:** Backend em Rust (crate do app Tauri em `src-tauri/`), exposto à interface por comandos `invoke` e eventos Tauri. whisper.cpp entra compilado via `whisper-rs`; FFmpeg e Codex CLI são sidecars. Frontend é HTML/CSS/JS puro em módulos ES servido de `ui/` sem bundler (`withGlobalTauri: true`); lógica pura testada com `node --test`.

**Tech Stack:** Rust stable, Tauri 2 (`tauri`, `tauri-plugin-dialog`, `tauri-plugin-shell`, `tauri-plugin-opener`), `whisper-rs`, `reqwest` (rustls, json, stream), `tokio`, `tokio-util` (CancellationToken), `serde`/`serde_json`, `thiserror`, `async-trait`, `keyring` 3 (`apple-native`, `windows-native`); dev: `wiremock`, `tempfile`. Frontend: JS puro, `marked` + `DOMPurify` vendorizados. Node apenas para `@tauri-apps/cli`.

**Spec:** `docs/superpowers/specs/2026-11-24-app-desktop-redesign-design.md`

## Global Constraints

- Identificador do app: `io.github.lucas1dev.localtranscriber`; nome exibido: `Local Transcriber`.
- Alvos: `aarch64-apple-darwin` e `x86_64-pc-windows-msvc`. Nada de Mac Intel ou Linux.
- Whisper: feature `metal` só no macOS; Windows somente CPU.
- FFmpeg redistribuído deve ser build **LGPL** (sem `--enable-gpl`).
- Frontend sem bundler e sem CDN: tudo em `ui/`, bibliotecas em `ui/vendor/`, funciona offline.
- Keychain: serviço `local-transcriber-ai`, conta = `openai` | `anthropic` | `gemini` (compatível com as chaves já salvas pelo app Python). Chave válida: 8 a 500 caracteres.
- Nunca devolver uma chave de API à interface; nunca ler, pedir ou guardar credenciais do ChatGPT ou do Claude. Login do ChatGPT só via `codex login` embutido; Claude Code só se a própria pessoa o tiver instalado e logado.
- Claude Code: presente em todos os builds, **desligado por padrão** (`claude_code_enabled = false`).
- Codex usa `CODEX_HOME = <dados do app>/codex-home`.
- Modelos Whisper: `<dados do app>/models`, baixados de `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-<id>.bin`, gravados como `.part` e renomeados só ao concluir.
- Catálogo Whisper (id · rótulo · bytes): `base` · Base — mais rápido · 147951465; `small` · Small — mais leve · 487601967; `medium` · Medium — maior qualidade · 1533763059; `large-v3-turbo` · Large V3 Turbo — padrão · 1624555275.
- Prompts, `SYSTEM`, formatos, prompts de trecho/consolidação, limite de 22000 **caracteres** (não bytes) por bloco e modelos padrão (`gpt-5.1`, `claude-sonnet-4-5`, `gemini-2.5-flash`, `qwen3:8b`) são copiados **literalmente** de `ai_engine.py`. Copie-os antes da Task 17, que apaga o arquivo.
- Ollama em `http://127.0.0.1:11434`.
- Textos de interface em português do Brasil. Erros aparecem dentro da tela, nunca em `alert()`.
- Toda dependência com versão exata fixada (`Cargo.lock` e `package-lock.json` versionados; URLs + SHA-256 dos sidecars fixados no script).
- Mensagens de commit e descrições de PR **sem** linha `Co-Authored-By`, sem menção a Claude/Claude Code e sem link ou ID de sessão. Só a mensagem.

## Review Focus

1. **Áudio sem fala (silêncio, música):** o whisper não devolve segmentos → o app fica no passo ① com "Nenhuma fala detectada no áudio." em vez de avançar com tela vazia ou dar erro. Testes: Task 4 (`join_text` vazio) e Task 11 (`afterTranscribe`).
2. **Caminhos com espaço, acento ou unicode** ("Reunião ação 1.m4a", pasta de usuário com acento no Windows) → passam intactos ao FFmpeg e aos sidecars. Teste: Task 4 (`ffmpeg_args_keeps_unicode_path`, `decodes_file_with_accented_name`).
3. **Download de modelo interrompido** (rede cai, servidor fecha antes do fim) → modelo não aparece como instalado, não sobra arquivo final corrompido, e "Baixar" funciona de novo. Teste: Task 3 (`truncated_download_is_not_installed`).
4. **Sair do passo durante um trabalho em andamento** (ir para Configurações ou voltar um passo enquanto transcreve ou resume) → o trabalho continua, o resultado não se perde e não dá para disparar um segundo em paralelo. Teste: Task 11 (`cannot_start_second_job`, `navigation_keeps_job`).
5. **Transcrição longa (2 h, > 100 mil caracteres) enviada ao Codex ou ao Claude Code** → o texto vai por stdin (o Windows limita a linha de comando a 32 mil caracteres) e chega inteiro. Teste: Task 8 (`long_prompt_goes_through_stdin`).

---

## Fase A — Fundação e spike

### Task 1: Repositório git e esqueleto Tauri

**Files:**
- Create: `.gitignore`, `package.json`, `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/Info.plist`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/error.rs`, `ui/index.html`, `ui/css/app.css`, `ui/js/app.js`

**Interfaces:**
- Produces: `AppError` em `src-tauri/src/error.rs`:
  ```rust
  #[derive(Debug, thiserror::Error)]
  pub enum AppError { #[error("{0}")] User(String), #[error("{0}")] Internal(String), #[error("Operação cancelada.")] Cancelled }
  ```
  Serializado para o frontend como `{ "kind": "user"|"internal"|"cancelled", "message": String }` (impl manual de `Serialize`). `impl From<std::io::Error>` e `From<reqwest::Error>` → `Internal`.
- Produces: `lib.rs` com `pub fn run()`, onde as tasks seguintes registram estado e comandos.

- [ ] **Step 1:** `git init`. Crie o `.gitignore` com: `.venv/`, `models/`, `outputs/`, `.superpowers/`, `node_modules/`, `src-tauri/target/`, `src-tauri/binaries/*` (mas não `src-tauri/binaries/.gitkeep`), `*.part`, `.DS_Store`. Depois rode `git remote add origin git@github.com:lucas1dev/localtranscriber.git`.
- [ ] **Step 2:** Rode `npm init -y` e `npm i -D @tauri-apps/cli@2` (versão exata no lock). Scripts: `"tauri": "tauri"` e `"test:ui": "node --test ui/tests/"`.
- [ ] **Step 3:** Configure o `tauri.conf.json`:
  - `identifier`: `io.github.lucas1dev.localtranscriber`, `productName` `Local Transcriber`.
  - `build.frontendDist`: `"../ui"`, sem `devUrl`.
  - `app.withGlobalTauri: true`, janela de 1100×760, mínimo de 900×640.
  - `bundle.targets`: `["app","dmg","msi","nsis"]`.
  - `bundle.macOS.infoPlist`: `Info.plist`, com `NSMicrophoneUsageDescription` = "O Local Transcriber usa o microfone para transcrever gravações no seu computador."
  - `bundle.externalBin` fica vazio por agora; a Task 15 preenche.

  Na capability `default.json`, inclua `core:default`, `dialog:default`, `opener:default` e `shell:allow-execute`.
- [ ] **Step 4:** No `Cargo.toml`, adicione as dependências do Tech Stack. Use `whisper-rs` sem features como dependência base, e em `[target.'cfg(target_os = "macos")'.dependencies]` coloque o mesmo `whisper-rs` com `features = ["metal"]`.
- [ ] **Step 5:** Crie `ui/index.html` com um `<h1>Local Transcriber</h1>`. Rode `npm run tauri dev`. Esperado: a janela abre e mostra o título.
- [ ] **Step 6:** Rode `cd src-tauri && cargo test`. Esperado: compila, 0 testes, OK.
- [ ] **Step 7:** Commit inicial com o código Python atual, `docs/` e o esqueleto. Depois rode `git push -u origin main`. Antes do push, confira com `git ls-remote origin` que o remoto está vazio. Se não estiver, pare e pergunte ao autor.

### Task 2: Pastas do app e preferências

**Files:**
- Create: `src-tauri/src/paths.rs`, `src-tauri/src/settings.rs`
- Test: testes `#[cfg(test)]` nos próprios arquivos

**Interfaces:**
- Produces (`paths.rs`):
  - `pub struct AppPaths { pub data_dir: PathBuf, pub models_dir: PathBuf, pub codex_home: PathBuf, pub settings_file: PathBuf }`
  - `impl AppPaths { pub fn from_data_dir(dir: PathBuf) -> std::io::Result<Self> }`: cria `models/` e `codex-home/`; `settings_file = dir/settings.json`.
  - `pub fn find_executable(name: &str, extra_dirs: &[PathBuf], path_var: Option<OsString>) -> Option<PathBuf>`: procura primeiro em `extra_dirs`, depois no PATH dado. No Windows, tenta também `name.exe` e `name.cmd`.
  - `pub fn claude_search_dirs(home: &Path) -> Vec<PathBuf>`:
    - macOS: `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`
    - Windows: `%USERPROFILE%\.local\bin`, `%APPDATA%\npm`
- Produces (`settings.rs`):
  - `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] #[serde(default)] pub struct Settings { pub default_model: String, pub default_language: String, pub summary_provider: Option<ProviderId>, pub summary_style: String, pub claude_code_enabled: bool }`
  - Valores padrão: `"large-v3-turbo"`, `"auto"`, `None`, `"executive"`, `false`. `ProviderId` vem da Task 6; até lá, use `Option<String>` e troque o tipo na Task 6.
  - `pub fn load(path: &Path) -> Settings`
  - `pub fn save(path: &Path, s: &Settings) -> Result<(), AppError>`: grava em `.tmp` e renomeia.

- [ ] **Step 1: Escreva os testes que falham**
  ```rust
  #[test] fn missing_file_gives_defaults() { assert_eq!(load(&tmp.join("nope.json")), Settings::default()); assert!(!Settings::default().claude_code_enabled); }
  #[test] fn corrupt_file_gives_defaults() { fs::write(&p, "{oops").unwrap(); assert_eq!(load(&p), Settings::default()); }
  #[test] fn partial_file_fills_defaults() { fs::write(&p, r#"{"default_language":"pt"}"#).unwrap(); let s = load(&p); assert_eq!(s.default_language, "pt"); assert_eq!(s.default_model, "large-v3-turbo"); }
  #[test] fn save_then_load_roundtrip() { /* altera default_model para "small", save, load, assert_eq */ }
  #[test] fn from_data_dir_creates_subdirs() { let p = AppPaths::from_data_dir(tmp.clone()).unwrap(); assert!(p.models_dir.is_dir() && p.codex_home.is_dir()); }
  #[test] fn find_executable_prefers_extra_dirs() { /* cria arquivo executável "claude" em extra e em outro dir do PATH; espera o de extra */ }
  #[test] fn find_executable_none_when_absent() { assert!(find_executable("nao-existe", &[], Some("".into())).is_none()); }
  ```
- [ ] **Step 2:** `cargo test paths settings`. Esperado: FAIL (funções não existem).
- [ ] **Step 3:** Implemente as funções nos dois arquivos.
- [ ] **Step 4:** `cargo test paths settings`. Esperado: PASS.
- [ ] **Step 5:** Commit `feat: pastas do app e preferências`.

### Task 3: Catálogo e download de modelos Whisper

**Files:**
- Create: `src-tauri/src/models.rs`
- Test: no próprio arquivo (usa `wiremock`, `tempfile`)

**Interfaces:**
- Consumes: `AppError`.
- Produces:
  - `pub struct ModelInfo { pub id: &'static str, pub label: &'static str, pub size_bytes: u64 }`
  - `pub const CATALOG: [ModelInfo; 4]`, com os valores das Global Constraints e nessa ordem: `large-v3-turbo`, `medium`, `small`, `base`.
  - `pub const HF_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main"`
  - `pub fn file_name(id: &str) -> String`, que devolve `ggml-<id>.bin`.
  - `pub fn find(id: &str) -> Option<&'static ModelInfo>`
  - `#[derive(Serialize)] pub struct ModelState { pub id: String, pub label: String, pub size_bytes: u64, pub installed: bool, pub is_default: bool }`
  - `pub fn list(models_dir: &Path, default_id: &str) -> Vec<ModelState>`. Conta como instalado só o arquivo final, nunca o `.part`.
  - `pub async fn download(http: &reqwest::Client, base_url: &str, models_dir: &Path, id: &str, cancel: CancellationToken, on_progress: impl Fn(u64, Option<u64>) + Send) -> Result<PathBuf, AppError>`. Grava em streaming num `.part`. Confere se os bytes recebidos batem com o `Content-Length`, quando ele existir. Só então renomeia. Se for cancelado, apaga o `.part` e devolve `Cancelled`. Limite o progresso a 1 evento a cada 250 ms.
  - `pub fn delete(models_dir: &Path, id: &str) -> Result<(), AppError>`
  - `pub fn import_file(src: &Path, models_dir: &Path) -> Result<String, AppError>`. Aceita só nomes `ggml-<id>.bin` com `id` no catálogo e devolve o `id`. Se o nome não bater: `User("Arquivo não reconhecido. Use um modelo ggml do catálogo (ex.: ggml-large-v3-turbo.bin).")`.

- [ ] **Step 1: Escreva os testes que falham**
  ```rust
  #[test] fn list_marks_installed_and_default() { /* cria ggml-small.bin; list(dir,"small") → small.installed && small.is_default; base.installed == false */ }
  #[test] fn part_file_is_not_installed() { /* cria ggml-base.bin.part; base.installed == false */ }
  #[tokio::test] async fn download_writes_file_and_reports_progress() { /* wiremock GET /ggml-base.bin → 10_000 bytes; download(...) Ok; arquivo final tem 10_000 bytes; on_progress chamado com total Some(10_000) */ }
  #[tokio::test] async fn truncated_download_is_not_installed() { /* mock com header Content-Length 10_000 e corpo de 4_000 bytes → Err; list(): base.installed == false; ggml-base.bin não existe; um novo download bem-sucedido em seguida funciona */ }
  #[tokio::test] async fn cancelled_download_cleans_part() { /* cancela o token antes de iniciar → Err(Cancelled); nenhum .part restante */ }
  #[test] fn import_rejects_unknown_name() { assert!(matches!(import_file(&tmp.join("foo.bin"), &dir), Err(AppError::User(_)))); }
  #[test] fn import_copies_known_model() { /* src ggml-small.bin em outro dir → Ok("small"); existe em models_dir */ }
  ```
  Para o teste de corpo truncado: se o wiremock não permitir um `Content-Length` maior que o corpo, use um `tokio::net::TcpListener` que escreve a resposta à mão e fecha a conexão.
- [ ] **Step 2:** `cargo test models`. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** `cargo test models`. Esperado: PASS.
- [ ] **Step 5:** Commit `feat: catálogo e download de modelos whisper`.

### Task 4: Motor de transcrição (FFmpeg + whisper-rs) — spike parte 1

**Files:**
- Create: `src-tauri/src/transcribe.rs`, `src-tauri/tests/fixtures/README.md` (explica como obter `jfk.wav` de `https://github.com/ggerganov/whisper.cpp/raw/master/samples/jfk.wav`), `scripts/fetch-test-fixtures.sh`
- Test: no próprio arquivo + `src-tauri/tests/transcribe_real.rs` (`#[ignore]`)

**Interfaces:**
- Produces:
  - `#[derive(Serialize, Clone, Debug, PartialEq)] pub struct Segment { pub start: String, pub end: String, pub text: String }`
  - `#[derive(Serialize, Clone, Debug)] pub struct Transcript { pub text: String, pub segments: Vec<Segment>, pub model: String, pub language: String, pub source_name: String }`
  - `pub fn format_ts(centis: i64) -> String`: recebe a unidade do whisper (10 ms) e devolve `HH:MM:SS,mmm`.
  - `pub fn ffmpeg_args(input: &Path) -> Vec<OsString>`, que devolve exatamente `-nostdin -hide_banner -loglevel error -i <input> -ac 1 -ar 16000 -f f32le -`.
  - `pub fn decode_to_pcm(ffmpeg: &Path, input: &Path) -> Result<Vec<f32>, AppError>`. Lê o stdout como f32 little-endian. Se o código de saída for diferente de 0: `User("Não foi possível ler o áudio: <últimos 600 caracteres do stderr>")`.
  - `pub fn join_text(segments: &[Segment]) -> String`: junta os textos já sem espaço nas pontas, separados por um espaço.
  - `pub struct Engine` com `impl Engine { pub fn new() -> Self; pub fn transcribe(&self, model_path: &Path, model_id: &str, pcm: &[f32], language: &str, cancel: Arc<AtomicBool>, on_progress: Box<dyn FnMut(i32) + Send>) -> Result<Vec<Segment>, AppError> }`.
    - Guarda em cache o `WhisperContext` do último `model_id`, dentro de um `Mutex`.
    - Idioma: `"auto"` vira detecção automática; qualquer outro código é repassado como está.
    - Usa o abort callback do whisper-rs ligado a `cancel`. Se for abortado, devolve `Cancelled`.

- [ ] **Step 1: Escreva os testes que falham**
  ```rust
  #[test] fn format_ts_examples() { assert_eq!(format_ts(0), "00:00:00,000"); assert_eq!(format_ts(123), "00:00:01,230"); assert_eq!(format_ts(366_050), "01:01:00,500"); }
  #[test] fn ffmpeg_args_keeps_unicode_path() { let p = Path::new("/tmp/Reunião ação 1.m4a"); let a = ffmpeg_args(p); assert_eq!(a[5], OsString::from("/tmp/Reunião ação 1.m4a")); assert_eq!(a.last().unwrap(), "-"); }
  #[test] fn join_text_empty_is_empty() { assert_eq!(join_text(&[]), ""); }
  #[test] fn join_text_trims_and_joins() { /* [" Olá ", "mundo "] → "Olá mundo" */ }
  ```
  Em `tests/transcribe_real.rs`, todos marcados com `#[ignore]` e lendo do ambiente `LT_FFMPEG` (caminho do ffmpeg) e `LT_MODEL` (caminho de um ggml):
  ```rust
  #[test] #[ignore] fn transcribes_jfk() { /* decode jfk.wav; transcribe "auto"; join_text contém "ask not what your country can do for you" (case-insensitive) */ }
  #[test] #[ignore] fn decodes_file_with_accented_name() { /* copia jfk.wav para tmp/"Reunião ação 1.wav"; decode_to_pcm Ok e len > 16000 */ }
  #[test] #[ignore] fn silence_gives_no_segments() { /* pcm = vec![0.0; 16000*5]; transcribe → join_text(..) == "" */ }
  #[test] #[ignore] fn cancel_aborts() { /* cancel=true antes; Err(Cancelled) */ }
  ```
- [ ] **Step 2:** `cargo test transcribe`. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** `cargo test transcribe`. Esperado: PASS.
- [ ] **Step 5:** No Mac, rode `scripts/fetch-test-fixtures.sh` e depois `LT_FFMPEG=$(which ffmpeg) LT_MODEL=$PWD/../models/ggml-large-v3-turbo.bin cargo test --test transcribe_real -- --ignored --nocapture`. Esperado: os 4 testes passam, e o log do whisper mostra `using Metal backend` ou equivalente. **Se o Metal não aparecer, pare e reporte.**
- [ ] **Step 6:** Commit `feat: motor de transcrição com whisper-rs`.

### Task 5: Comandos de transcrição + microfone na webview — spike parte 2

**Files:**
- Create: `src-tauri/src/state.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/src/commands/transcription.rs`, `ui/spike-mic.html` (temporário; apagado na Task 13)
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `AppPaths`, `Settings`, `models::*`, `transcribe::*`.
- Produces (`state.rs`):
  ```rust
  pub struct Sidecars { pub ffmpeg: PathBuf, pub codex: PathBuf }
  pub struct AppState { pub paths: AppPaths, pub settings: Mutex<Settings>, pub engine: Arc<Engine>, pub http: reqwest::Client, pub sidecars: Sidecars,
                        pub transcription_cancel: Mutex<Option<Arc<AtomicBool>>>, pub downloads: Mutex<HashMap<String, CancellationToken>> }
  ```
  Resolução do sidecar: `ffmpeg` e `codex` ficam ao lado do executável do app (padrão do Tauri para `externalBin`). Em `tauri dev` sem sidecar, caia para `find_executable` no PATH.
- Produces (comandos, nomes exatos):
  - `transcribe_file(path: String, model: String, language: String) -> Result<Transcript, AppError>`. Roda em `spawn_blocking` e emite `transcribe://progress` `{ percent: i32 }`. Se já houver uma transcrição rodando: `User("Já existe uma transcrição em andamento.")`. Se o modelo não estiver instalado: `User("Modelo <label> não instalado. Baixe em Configurações → Transcrição.")`.
  - `transcribe_chunk(data: Vec<u8>, mime: String, model: String, language: String) -> Result<Transcript, AppError>`. Grava num arquivo temporário (`.webm` ou `.m4a` conforme o mime) e segue o mesmo caminho. Não usa o bloqueio de "já existe", porque os blocos chegam em fila.
  - `cancel_transcription() -> ()`

- [ ] **Step 1:** Implemente `AppState`, os 3 comandos e o registro no `lib.rs`. `AppState` é montado em `setup()` com `app.path().app_data_dir()`.
- [ ] **Step 2:** `cargo test`. Esperado: PASS (nada quebrou).
- [ ] **Step 3:** Crie o `ui/spike-mic.html`. Ele deve:
  - ter um botão Gravar que usa `getUserMedia` + `MediaRecorder` em blocos de 12 s;
  - mandar cada bloco por `invoke('transcribe_chunk', {data: Array.from(new Uint8Array(await blob.arrayBuffer())), mime, model: 'large-v3-turbo', language: 'auto'})`;
  - escrever o texto na página;
  - ter também um seletor de arquivo que chama `transcribe_file` mostrando o progresso.

  Para testar, aponte o `index.html` temporariamente para ele.
- [ ] **Step 4: Verificação manual no Mac.** Copie ou baixe o modelo para a pasta de dados e rode `npm run tauri dev`. Esperado:
  - o macOS pede permissão de microfone;
  - 2 blocos de fala viram texto;
  - o arquivo `jfk.wav` transcreve com a barra de progresso passando de 0 a 100.

  **Se o microfone não funcionar na webview do Mac**, faça a Task 5b antes de seguir.
- [ ] **Step 5: Verificação manual no Windows** (máquina ou VM com Windows 11, após `rustup`, VS Build Tools, CMake e LLVM): o mesmo roteiro do Step 4. Se não houver Windows disponível agora, registre como pendente no PR e valide pelo artefato da Task 16.
- [ ] **Step 6:** Commit `feat: comandos de transcrição e spike de microfone`.

### Task 5b (condicional): Gravação nativa com `cpal`

Só se o Step 4 ou o Step 5 da Task 5 falhar.

**Files:** Create `src-tauri/src/recorder.rs`.

**Interfaces:**
- Produces os comandos `mic_start(model: String, language: String) -> ()` e `mic_stop() -> ()`.
- Emite `mic://chunk` com `Transcript` a cada 12 s de áudio.
- Captura com `cpal` e reamostra para 16 kHz mono em f32, sem passar pelo FFmpeg.

O frontend (Task 12) usa esses comandos no lugar do `MediaRecorder`; `ui/js/mic.js` esconde essa diferença atrás da mesma interface.

- [ ] **Step 1:** Escreva o teste `resample_48k_to_16k_len`: 48000 amostras viram 16000 ± 1.
- [ ] **Step 2:** Rode o teste. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** Rode o teste. Esperado: PASS.
- [ ] **Step 5:** Repita a verificação manual da Task 5.
- [ ] **Step 6:** Commit.

## Fase B — Backend

### Task 6: Credenciais e identificadores de provedor

**Files:**
- Create: `src-tauri/src/credentials.rs`, `src-tauri/src/providers/mod.rs`
- Modify: `src-tauri/src/settings.rs` (`summary_provider: Option<ProviderId>`)

**Interfaces:**
- Produces (`providers/mod.rs`):
  - `#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)] #[serde(rename_all = "snake_case")] pub enum ProviderId { Codex, Openai, Anthropic, Gemini, Ollama, ClaudeCode }`
  - `pub fn label(id) -> &'static str`:

    | `ProviderId` | rótulo |
    |---|---|
    | `Codex` | "ChatGPT (assinatura)" |
    | `Openai` | "OpenAI (API)" |
    | `Anthropic` | "Claude (API)" |
    | `Gemini` | "Gemini (API)" |
    | `Ollama` | "Ollama (local)" |
    | `ClaudeCode` | "Claude Code (assinatura)" |
  - `pub fn default_model(id) -> Option<&'static str>`: `Openai` `gpt-5.1`, `Anthropic` `claude-sonnet-4-5`, `Gemini` `gemini-2.5-flash`, `Ollama` `qwen3:8b`. `Codex` e `ClaudeCode` devolvem `None` (cada um usa o padrão da própria CLI).
  - `pub fn keychain_account(id) -> Option<&'static str>`: `Openai` `"openai"`, `Anthropic` `"anthropic"`, `Gemini` `"gemini"`; os demais devolvem `None`.
- Produces (`credentials.rs`):
  - `pub const SERVICE: &str = "local-transcriber-ai";`
  - `pub trait SecretStore: Send + Sync { fn get(&self, account: &str) -> Result<Option<String>, AppError>; fn set(&self, account: &str, secret: &str) -> Result<(), AppError>; fn delete(&self, account: &str) -> Result<(), AppError>; }`
  - `pub struct KeyringStore;`: usa `keyring::Entry::new(SERVICE, account)`. `NoEntry` vira `Ok(None)` e excluir algo que não existe vira `Ok(())`.
  - `pub struct MemoryStore(Mutex<HashMap<String, String>>)`: para testes, em `#[cfg(any(test, feature = "test-util"))]`.
  - `pub fn validate_key(key: &str) -> Result<(), AppError>`: aplica `trim` e exige de 8 a 500 caracteres. Se falhar: `User("A chave deve ter entre 8 e 500 caracteres.")`.

- [ ] **Step 1: Escreva os testes que falham**
  ```rust
  #[test] fn provider_id_serializes_snake_case() { assert_eq!(serde_json::to_string(&ProviderId::ClaudeCode).unwrap(), "\"claude_code\""); }
  #[test] fn keychain_accounts_match_python_app() { assert_eq!(keychain_account(ProviderId::Openai), Some("openai")); assert_eq!(keychain_account(ProviderId::Codex), None); }
  #[test] fn validate_key_bounds() { assert!(validate_key("1234567").is_err()); assert!(validate_key("12345678").is_ok()); assert!(validate_key(&"x".repeat(501)).is_err()); }
  #[test] fn memory_store_roundtrip_and_delete_missing_ok() { /* set/get/delete/delete */ }
  #[test] #[ignore] fn keyring_reads_python_saved_key() { /* manual: LT_EXPECT_KEY_ACCOUNT=anthropic → KeyringStore.get(...) é Some */ }
  ```
- [ ] **Step 2:** `cargo test credentials providers`. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** `cargo test credentials providers`. Esperado: PASS.
- [ ] **Step 5:** Commit `feat: credenciais no keychain e ids de provedor`.

### Task 7: Núcleo de resumo (prompts, divisão, map-reduce)

**Files:**
- Create: `src-tauri/src/summarize.rs`

**Interfaces:**
- Consumes: `AppError`.
- Produces:
  - `pub const SYSTEM: &str`, `pub fn style_prompt(style: &str) -> Option<&'static str>`: as 7 chaves `executive`, `detailed`, `academic`, `meeting`, `class`, `podcast`, `custom`, com o texto literal de `ai_engine.py`.
  - `pub const CHUNK_LIMIT: usize = 22_000;`
  - `pub fn split_text(text: &str, limit: usize) -> Vec<String>`: port fiel de `_split_text`, contando **caracteres** (`char`), nunca bytes.
  - `pub fn user_message(prompt: &str, text: &str) -> String`, que devolve `format!("{prompt}\n\nTRANSCRIÇÃO:\n{text}")`.
  - `#[async_trait] pub trait Llm: Send + Sync { async fn complete(&self, system: &str, user: &str) -> Result<String, AppError>; }`
  - `pub async fn run_summary(llm: &dyn Llm, text: &str, style: &str, custom_instructions: &str) -> Result<(String, usize), AppError>`: devolve `(resumo, blocos_processados)`. Mesmas validações e mensagens do Python: "A transcrição está vazia" e "Formato de resumo inválido". Usa os prompts literais de trecho e de consolidação.

- [ ] **Step 1: Escreva os testes que falham**, com um `FakeLlm` que registra as chamadas e devolve `"R<n>"`:
  ```rust
  #[test] fn split_short_text_single_chunk() { assert_eq!(split_text("a\nb", 22_000), vec!["a\nb"]); }
  #[test] fn split_counts_chars_not_bytes() { let t = "ã".repeat(30); assert_eq!(split_text(&t, 20).iter().map(|c| c.chars().count()).sum::<usize>(), 30); }
  #[test] fn split_prefers_sentence_boundary() { let t = format!("{}. {}", "a".repeat(15), "b".repeat(15)); let c = split_text(&t, 20); assert_eq!(c[0], format!("{}.", "a".repeat(15))); }
  #[test] fn split_drops_blank_lines() { assert_eq!(split_text("a\n\n\nb", 100), vec!["a\nb"]); }
  #[tokio::test] async fn single_chunk_one_call() { /* run_summary(texto curto) → 1 chamada; user começa com o style prompt de "executive" */ }
  #[tokio::test] async fn map_reduce_calls_n_plus_one() { /* texto de 50_000 chars em parágrafos de 1_000 → 3 trechos + 1 consolidação = 4 chamadas; última user começa com "Consolide as sínteses parciais"; retorno .1 == 3 */ }
  #[tokio::test] async fn custom_instructions_appended_only_for_custom() { /* style custom + "foco em prazos" → user contém "Instruções do usuário: foco em prazos"; style executive + instruções → não contém */ }
  #[tokio::test] async fn empty_text_is_user_error() { /* "   " → Err(User("A transcrição está vazia")) */ }
  #[tokio::test] async fn invalid_style_is_user_error() { /* "xyz" → Err(User("Formato de resumo inválido")) */ }
  ```
  Antes de implementar, confira os casos de `split_*` rodando `_split_text` do Python com as mesmas entradas e ajuste as expectativas para baterem.
- [ ] **Step 2:** `cargo test summarize`. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** `cargo test summarize`. Esperado: PASS.
- [ ] **Step 5:** Commit `feat: núcleo de resumo com map-reduce`.

### Task 8: Provedores — HTTP (OpenAI, Anthropic, Gemini, Ollama) e CLI (Codex, Claude Code)

**Files:**
- Create: `src-tauri/src/providers/http.rs`, `src-tauri/src/providers/cli.rs`, `src-tauri/src/providers/codex.rs`, `src-tauri/src/providers/claude_code.rs`, `src-tauri/src/providers/ollama.rs`
- Modify: `src-tauri/src/providers/mod.rs`

**Interfaces:**
- Consumes: `Llm`, `SYSTEM`, `user_message`, `SecretStore`, `ProviderId`, `find_executable`, `claude_search_dirs`.
- Produces (`mod.rs`):
  - `pub struct Endpoints { pub openai: String, pub anthropic: String, pub gemini: String, pub ollama: String }`, com `Default` = `https://api.openai.com`, `https://api.anthropic.com`, `https://generativelanguage.googleapis.com`, `http://127.0.0.1:11434`.
  - `pub struct ProviderCtx { pub http: reqwest::Client, pub endpoints: Endpoints, pub secrets: Arc<dyn SecretStore>, pub codex: codex::Codex, pub claude: Option<claude_code::ClaudeCode> }`
  - `pub fn build(id: ProviderId, model: Option<String>, ctx: &ProviderCtx) -> Result<Box<dyn Llm>, AppError>`. Usa `default_model` quando `model` é `None`.

    | Situação | Mensagem (`User`) |
    |---|---|
    | Chave de API ausente | "Adicione uma chave de API <rótulo> em Configurações → Contas de IA." |
    | `ClaudeCode` com `claude` `None` | "Claude Code não encontrado neste computador." |
- Produces (`http.rs`): `OpenAi`, `Anthropic`, `Gemini`, cada um `{ http, base, key, model }` com `impl Llm`. Corpo, cabeçalhos e leitura da resposta são iguais aos de `ai_engine.py`, com timeout de 240 s. Para HTTP diferente de 2xx: `Internal("<rótulo> respondeu <status>: <primeiros 700 caracteres do corpo>")`.
- Produces (`ollama.rs`):
  - `Ollama { http, base, model }` com `impl Llm` via `/api/chat`, igual ao Python, com timeout de 600 s.
  - `pub async fn list_models(http, base) -> Result<Vec<String>, AppError>` (`/api/tags`)
  - `pub async fn is_running(http, base) -> bool` (timeout de 4 s)
  - `pub async fn pull(http, base, name, cancel: CancellationToken, on_progress: impl Fn(String, Option<u64>, Option<u64>) + Send) -> Result<(), AppError>`: `/api/pull` em streaming NDJSON, repassando `status`, `completed` e `total`. Uma linha com `error` vira `User(<error>)`.
  - `pub async fn delete(http, base, name) -> Result<(), AppError>` (`DELETE /api/delete`)
- Produces (`cli.rs`):
  - `pub struct CliOutput { pub code: i32, pub stdout: String, pub stderr: String }`
  - `pub async fn run(program: &Path, args: &[OsString], envs: &[(OsString, OsString)], stdin: Option<&str>, timeout: Duration) -> Result<CliOutput, AppError>`. Usa `tokio::process` com stdin em pipe. No timeout, mata o processo e devolve `User("<programa> não respondeu em <n> s.")`. No Windows, usa `CREATE_NO_WINDOW`.
- Produces (`codex.rs`):
  - `pub struct Codex { pub bin: PathBuf, pub home: PathBuf }`: toda chamada recebe `CODEX_HOME=home`.
  - `pub enum AuthStatus { LoggedIn(String), LoggedOut, Missing }`, serializado como `{state: "logged_in"|"logged_out"|"missing", detail}`.
  - `pub async fn status(&self) -> AuthStatus`: `codex login status`. Código 0 vira `LoggedIn(stdout sem espaços nas pontas)`, outro código vira `LoggedOut`, binário ausente vira `Missing`.
  - `pub async fn login(&self) -> Result<AuthStatus, AppError>`: `codex login`, que abre o navegador. Timeout de 10 min.
  - `pub async fn logout(&self) -> Result<(), AppError>`
  - `impl Llm for CodexLlm { bin, home, model: Option<String> }`: `codex exec` não interativo, só leitura, sem exigir repositório git, com o diretório de trabalho numa pasta temporária vazia e a resposta final gravada num arquivo temporário lido no fim. O prompt vai **por stdin**, formado por `SYSTEM + "\n\n" + user`. O modelo é passado só se vier `Some`. Timeout de 15 min. Confira as flags exatas na versão fixada do Codex (`codex exec --help`) e registre-as em comentário.
  - Se a saída indicar que o login é necessário: `User("Entre com sua conta ChatGPT em Configurações → Contas de IA.")`.
- Produces (`claude_code.rs`):
  - `pub struct ClaudeCode { pub bin: PathBuf }`
  - `pub fn locate(home: &Path, path_var: Option<OsString>) -> Option<ClaudeCode>`
  - `pub async fn status(&self) -> AuthStatus`: `claude auth status`, lendo `loggedIn` do JSON.
  - `impl Llm for ClaudeCodeLlm { bin, model }`: `claude -p --output-format text --no-session-persistence --system-prompt <SYSTEM>`, sem ferramentas (confira a flag com `claude --help` na versão instalada), `--model` só se vier `Some`, prompt por stdin, timeout de 15 min.
  - Nunca roda `claude login` nem lê arquivos de credencial.

- [ ] **Step 1: Escreva os testes que falham.** Os testes de HTTP usam `wiremock` com `Endpoints` apontando para o mock. Os testes de CLI usam scripts falsos `#!/bin/sh` em `tempfile` com `chmod +x`, marcados `#[cfg(unix)]`.
  ```rust
  #[tokio::test] async fn openai_sends_responses_payload_and_parses_output_text() { /* mock POST /v1/responses, Bearer K, body_partial_json {"model":"gpt-5.1","instructions":SYSTEM}; responde output[0].content[0]{type:"output_text",text:"ok"} → "ok" */ }
  #[tokio::test] async fn anthropic_sends_headers_and_parses_text() { /* x-api-key, anthropic-version 2023-06-01, max_tokens 6000 */ }
  #[tokio::test] async fn gemini_uses_header_key_and_parses_parts() { /* POST /v1beta/models/gemini-2.5-flash:generateContent, x-goog-api-key */ }
  #[tokio::test] async fn ollama_chat_parses_message_content() { }
  #[tokio::test] async fn http_error_includes_status_and_body() { /* 401 "bad key" → Err Internal contendo "401" e "bad key" */ }
  #[tokio::test] async fn build_without_key_is_user_error() { /* MemoryStore vazio, build(Openai) → User contendo "Configurações → Contas de IA" */ }
  #[tokio::test] async fn ollama_pull_streams_progress_and_surfaces_error() { /* NDJSON 2 linhas de progresso + {"error":"not found"} → on_progress chamado 2x; Err(User("not found")) */ }
  #[cfg(unix)] #[tokio::test] async fn long_prompt_goes_through_stdin() { /* script: `wc -c` do stdin → stdout; CodexLlm.complete com user de 120_000 chars; resultado numérico ≥ 120_000 e nenhum argumento contém o texto */ }
  #[cfg(unix)] #[tokio::test] async fn codex_receives_codex_home() { /* script imprime $CODEX_HOME; == home */ }
  #[cfg(unix)] #[tokio::test] async fn codex_status_maps_exit_codes() { /* exit 0 "Logged in using ChatGPT" → LoggedIn; exit 1 → LoggedOut; bin inexistente → Missing */ }
  #[cfg(unix)] #[tokio::test] async fn cli_timeout_kills_and_reports() { /* script `sleep 5`, timeout 200ms → Err(User) contendo "não respondeu" */ }
  #[test] fn claude_locate_finds_in_local_bin() { /* home/.local/bin/claude executável → Some */ }
  #[test] fn claude_locate_none_when_missing() { }
  ```
- [ ] **Step 2:** `cargo test providers`. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** `cargo test providers`. Esperado: PASS.
- [ ] **Step 5:** Verificação manual (opcional, com o Claude Code logado): rode `LT_REAL_CLAUDE=1 cargo test claude_real -- --ignored` com um teste `#[ignore]` que resume "Olá, isto é um teste." e espera uma resposta não vazia.
- [ ] **Step 6:** Commit `feat: provedores http e cli`.

### Task 9: Comandos de modelos, provedores, resumo e configurações

**Files:**
- Create: `src-tauri/src/commands/models.rs`, `src-tauri/src/commands/providers.rs`, `src-tauri/src/commands/summary.rs`, `src-tauri/src/commands/app.rs`
- Modify: `src-tauri/src/state.rs` (adiciona `secrets: Arc<dyn SecretStore>`, `endpoints: Endpoints`, `ollama_pulls: Mutex<HashMap<String, CancellationToken>>`), `src-tauri/src/lib.rs`

**Interfaces** (nomes exatos dos comandos; todos devolvem `Result<_, AppError>`):

- **Status e configurações** (`app.rs`)
  - `get_status() -> StatusDto { ffmpeg_ok: bool, codex_ok: bool, models: Vec<ModelState>, version: String, data_dir: String, versions: HashMap<String, String> }`. O campo `versions` traz `ffmpeg`, `codex` e `whisper`; vem `"?"` quando não der para ler.
  - `get_settings() -> Settings`
  - `set_settings(settings: Settings) -> Settings`
  - `open_data_dir() -> ()` (via plugin opener)
- **Modelos Whisper** (`models.rs`)
  - `list_models() -> Vec<ModelState>`
  - `download_model(id: String) -> ()`: volta na hora e roda numa task. Emite `model://progress` `{id, done, total}` e, no fim, `model://done` `{id, ok: bool, error: Option<AppError>}`.
  - `cancel_download(id: String)`
  - `delete_model(id)`
  - `set_default_model(id) -> Settings`
  - `import_model(path: String) -> String`
- **Contas de IA** (`providers.rs`)
  - `providers_status() -> Vec<ProviderStatusDto>`
    - Formato: `ProviderStatusDto { id: ProviderId, label, kind: "subscription"|"api_key"|"local", configured: bool, enabled: bool, detail: String }`.
    - Ordem fixa: `Codex`, `ClaudeCode`, `Openai`, `Anthropic`, `Gemini`, `Ollama`.
    - `ClaudeCode.enabled = settings.claude_code_enabled`; os demais vêm sempre com `enabled: true`.
    - `configured` de cada provedor:

      | Provedor | `configured` é verdadeiro quando |
      |---|---|
      | `Codex` | `codex login status` devolve `LoggedIn` |
      | `ClaudeCode` | `enabled`, `claude` localizado e logado |
      | `Openai`, `Anthropic`, `Gemini` | há chave salva |
      | `Ollama` | o Ollama está rodando e tem ao menos 1 modelo |
  - `save_api_key(provider: ProviderId, key: String) -> ProviderStatusDto`
  - `delete_api_key(provider) -> ()`
  - `codex_login() -> ProviderStatusDto`
  - `codex_logout() -> ()`
  - `ollama_models() -> OllamaDto { running: bool, models: Vec<String> }`
  - `ollama_pull(name)`: emite `ollama://progress` `{name, status, completed, total}` e `ollama://done` `{name, ok, error}`.
  - `cancel_ollama_pull(name)`
  - `ollama_delete(name)`
- **Resumo** (`summary.rs`)
  - `summarize(req: SummaryRequest { text, provider: ProviderId, style, model: Option<String>, custom_instructions }) -> SummaryResult { provider, model: Option<String>, style, summary, chunks_processed }`
  - `compare(req: CompareRequest { text, providers: Vec<ProviderId>, style, models: HashMap<ProviderId, String>, custom_instructions }) -> Vec<CompareItem { provider, model: Option<String>, summary: Option<String>, error: Option<AppError> }>`
    - Recebe de 2 a 4 provedores; fora disso: `User("Escolha de 2 a 4 provedores para comparar.")`.
    - Roda tudo em paralelo; um erro não derruba os outros.

- [ ] **Step 1: Escreva os testes que falham.** Extraia a lógica pura para funções testáveis sem Tauri:
  ```rust
  // providers.rs
  pub fn assemble_status(codex: &AuthStatus, claude: Option<&AuthStatus>, claude_enabled: bool, keys: &HashSet<ProviderId>, ollama: &OllamaDto) -> Vec<ProviderStatusDto>
  #[test] fn status_order_and_kinds() { /* ids na ordem fixa; Codex kind "subscription", Openai "api_key", Ollama "local" */ }
  #[test] fn claude_code_disabled_is_not_configured_even_if_logged_in() { }
  #[test] fn ollama_running_without_models_is_not_configured() { /* detail "Ollama rodando, nenhum modelo baixado" */ }
  // summary.rs
  pub fn validate_compare(providers: &[ProviderId]) -> Result<(), AppError>
  #[test] fn compare_requires_2_to_4() { /* 1 → Err; 2 → Ok; 5 → Err; duplicados contam uma vez */ }
  #[tokio::test] async fn compare_isolates_errors() { /* run_compare com builder injetável: um FakeLlm falha, outro ok → 2 itens, um com error, outro com summary */ }
  ```
  Para isso, `run_compare` recebe `impl Fn(ProviderId) -> Result<Box<dyn Llm>, AppError>`.
- [ ] **Step 2:** `cargo test commands`. Esperado: FAIL.
- [ ] **Step 3:** Implemente os comandos e registre-os todos em `lib.rs` (`tauri::generate_handler!`).
- [ ] **Step 4:** `cargo test`. Esperado: PASS (todos os testes não ignorados).
- [ ] **Step 5:** Commit `feat: comandos de modelos, provedores e resumo`.

## Fase C — Interface

Estilo visual (cabe a quem constrói os detalhes): tema escuro atual, verde `#b7f36b` como acento, tokens em `:root` em `ui/css/app.css`. Defina `--input` (o CSS antigo usava sem definir). Fonte monoespaçada: `ui-monospace, SFMono-Regular, Menlo, Consolas, monospace` (sem "DM Mono").

### Task 10: Base do frontend (API, estado, exportação, Markdown)

**Files:**
- Create: `ui/js/api.js`, `ui/js/store.js`, `ui/js/export.js`, `ui/js/markdown.js`, `ui/vendor/marked.min.js`, `ui/vendor/purify.min.js`, `ui/tests/store.test.js`, `ui/tests/export.test.js`
- Modify: `ui/index.html` (carrega `vendor/*` como scripts clássicos e `js/app.js` como `type="module"`)

**Interfaces:**
- Produces (`api.js`): `export const api = { getStatus, getSettings, setSettings, listModels, downloadModel, cancelDownload, deleteModel, setDefaultModel, importModel, transcribeFile, transcribeChunk, cancelTranscription, providersStatus, saveApiKey, deleteApiKey, codexLogin, codexLogout, ollamaModels, ollamaPull, cancelOllamaPull, ollamaDelete, summarize, compare, openDataDir, pickAudioFile, pickModelFile }`. Cada um é um wrapper fino de `window.__TAURI__.core.invoke`, com argumentos em camelCase, que o Tauri converte para snake_case. `pickAudioFile` e `pickModelFile` usam `window.__TAURI__.dialog.open` com filtros (áudio: `mp3 wav m4a mp4 mov webm ogg flac aac`; modelo: `bin`). Exporta também `export function on(event, cb) -> Promise<unlisten>` e `export function errorMessage(e) -> string`, que devolve `e.message` quando houver e senão `String(e)`.
- Produces (`store.js`): `export function createStore(initial) -> { get(), set(patch), subscribe(fn) -> unsubscribe }`. `set` faz um merge raso e notifica.
- Produces (`export.js`): `export function toSrt(segments) -> string` (mesmo formato do app atual: índice, `start --> end` com vírgula, texto, blocos separados por linha em branco). `export function downloadText(name, content)`.
- Produces (`markdown.js`): `export function renderMarkdown(md) -> string`, que devolve `DOMPurify.sanitize(marked.parse(md))`.

- [ ] **Step 1: Escreva os testes que falham** (`node:test`, `assert/strict`):
  ```js
  test('toSrt formata índice, intervalo e texto', () => assert.equal(toSrt([{start:'00:00:00,000',end:'00:00:02,500',text:'Olá'},{start:'00:00:02,500',end:'00:00:04,000',text:'mundo'}]), '1\n00:00:00,000 --> 00:00:02,500\nOlá\n\n2\n00:00:02,500 --> 00:00:04,000\nmundo'));
  test('toSrt vazio', () => assert.equal(toSrt([]), ''));
  test('store notifica e faz merge raso', () => { const s = createStore({a:1,b:2}); let seen; s.subscribe(v => seen = v); s.set({b:3}); assert.deepEqual(seen, {a:1,b:3}); });
  test('unsubscribe para de notificar', () => { /* ... */ });
  ```
- [ ] **Step 2:** `npm run test:ui`. Esperado: FAIL.
- [ ] **Step 3:** Implemente e baixe `marked` e `DOMPurify` em versões exatas para `ui/vendor/`. Registre as versões num comentário no topo de cada arquivo.
- [ ] **Step 4:** `npm run test:ui`. Esperado: PASS.
- [ ] **Step 5:** Commit `feat(ui): base do frontend`.

### Task 11: Lógica de navegação dos passos

**Files:**
- Create: `ui/js/flow.js`, `ui/tests/flow.test.js`

**Interfaces:**
- Produces: o estado inicial e as funções puras de transição. Nenhuma toca no DOM.
  - Estado inicial: `export const initialState = { view: 'steps', step: 1, maxStep: 1, transcript: null, edited: false, job: null, error: null, summaries: [] }`
    - `transcript`: `{text, segments, model, language, sourceName}`
    - `job`: `{kind: 'transcribe'|'mic'|'summary'|'compare', percent: number|null}`
  - `export function goTo(state, step) -> patch`: só permite `step <= maxStep`; senão devolve `{}`.
  - `export function openSettings(state) -> patch` / `closeSettings(state) -> patch`: mudam só `view` e preservam todo o resto.
  - `export function canStartJob(state) -> boolean`: `state.job === null`.
  - `export function startJob(state, kind) -> patch`: se já houver job, devolve `{error: 'Aguarde o processamento atual terminar.'}`.
  - `export function afterTranscribe(state, transcript) -> patch`
    - Com `segments.length === 0` e `text` vazio: `{job: null, step: 1, error: 'Nenhuma fala detectada no áudio.'}`.
    - Senão: `{job: null, transcript, edited: false, step: 2, maxStep: Math.max(state.maxStep, 2), summaries: [], error: null}`.
  - `export function pasteText(state, text) -> patch`: cria um transcript `{text, segments: [], model: null, language: null, sourceName: 'Texto colado'}` e vai para o passo 2.
  - `export function needsReplaceConfirm(state) -> boolean`: `state.transcript !== null && state.edited`.
  - `export function failJob(state, message) -> patch`: devolve `{job: null, error: message}`.

- [ ] **Step 1: Escreva os testes que falham**
  ```js
  test('não pula para passo não alcançado', () => assert.deepEqual(goTo({...initialState}, 3), {}));
  test('silêncio mantém no passo 1 com erro', () => assert.deepEqual(afterTranscribe({...initialState, job:{kind:'transcribe'}}, {text:'', segments:[]}), {job:null, step:1, error:'Nenhuma fala detectada no áudio.'}));
  test('transcrição avança e libera passo 2', () => { const p = afterTranscribe({...initialState}, {text:'oi', segments:[{start:'',end:'',text:'oi'}]}); assert.equal(p.step, 2); assert.equal(p.maxStep, 2); });
  test('cannot_start_second_job', () => { const s = {...initialState, job:{kind:'transcribe', percent:10}}; assert.equal(canStartJob(s), false); assert.equal(startJob(s,'summary').error, 'Aguarde o processamento atual terminar.'); });
  test('navigation_keeps_job', () => { const s = {...initialState, step:2, maxStep:3, job:{kind:'summary', percent:null}}; const p1 = openSettings(s); assert.equal(p1.job, undefined); assert.deepEqual(goTo({...s, ...p1}, 1), {step:1}); });
  test('texto editado pede confirmação antes de substituir', () => assert.equal(needsReplaceConfirm({...initialState, transcript:{text:'x'}, edited:true}), true));
  test('colar texto vai ao passo 2', () => assert.equal(pasteText(initialState, 'abc').step, 2));
  ```
- [ ] **Step 2:** `npm run test:ui`. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** `npm run test:ui`. Esperado: PASS.
- [ ] **Step 5:** Commit `feat(ui): lógica dos passos`.

### Task 12: Tela principal — cabeçalho, passo ① Áudio e microfone

**Files:**
- Create: `ui/js/app.js` (monta tudo), `ui/js/views/header.js`, `ui/js/views/step-audio.js`, `ui/js/mic.js`, `ui/js/views/error-banner.js`
- Modify: `ui/index.html`, `ui/css/app.css`

**Interfaces:**
- Consumes: `api`, `createStore`, `flow.*`.
- Produces: `export function mountX(root, store)` em cada view. `mic.js` exporta `createMic({onChunkText, onError}) -> { start(model, language), stop() -> Promise<void> }`. Por baixo, usa `MediaRecorder` + `transcribeChunk` (blocos de 12 s, fila sequencial, como no `static/app.js`), ou `mic_start`/`mic_stop` se a Task 5b tiver sido feita.

**Comportamento:**

- **Cabeçalho**
  - Mostra `① Áudio ─ ② Transcrição ─ ③ Resumo`. Os passos `≤ maxStep` são clicáveis, via `goTo`.
  - O botão ⚙ chama `openSettings`.
  - Um indicador de job aparece em qualquer view enquanto `job !== null`.
- **Passo ① Áudio**
  - **Abas** Arquivo | Microfone.
  - **Arquivo:** a área de soltar aceita `tauri://drag-drop` (usa o primeiro caminho) e clique, que chama `pickAudioFile`. Mostra o nome e o tamanho do arquivo escolhido.
  - **Modelo e idioma:** os valores iniciais vêm de `getSettings`. Modelos não instalados aparecem com `disabled` e o sufixo " — não instalado". Ao lado, um link "Baixar modelos" abre Configurações → Transcrição.
  - **Transcrever:**
    - Antes de começar, se `needsReplaceConfirm` for verdadeiro, pede confirmação dentro da tela: "Substituir a transcrição editada?" com botões Substituir e Cancelar.
    - Durante o trabalho, mostra uma barra com o percentual de `transcribe://progress` e um botão Cancelar (`cancelTranscription`).
    - Ao terminar, aplica `afterTranscribe`.
  - **Microfone:** mostra o tempo decorrido e o texto ao vivo. Ao finalizar, espera a fila e aplica `afterTranscribe` com o texto acumulado e os segmentos concatenados.
  - **Link "Já tenho o texto":** abre um textarea com o botão "Usar este texto", que chama `pasteText`.
  - **Primeira abertura sem nenhum modelo instalado:** um aviso no topo do passo com o botão "Abrir Configurações".
- **Erros:** o `error-banner` mostra `state.error`, tem um botão de fechar e é limpo ao trocar de passo.

- [ ] **Step 1:** Implemente as views e o CSS.
- [ ] **Step 2:** `npm run test:ui && (cd src-tauri && cargo test)`. Esperado: PASS (nada quebrou).
- [ ] **Step 3: Verificação manual** (`npm run tauri dev`). Esperado:
  - (a) Sem modelos instalados, o aviso aparece e Transcrever fica desabilitado.
  - (b) Com modelo, arrastar `jfk.wav` mostra o nome; Transcrever mostra a barra e depois vai para o passo 2.
  - (c) Um WAV de 5 s de silêncio (`ffmpeg -f lavfi -i anullsrc=r=16000:cl=mono -t 5 silencio.wav`) mantém no passo 1 com "Nenhuma fala detectada no áudio.".
  - (d) Cancelar no meio volta ao estado parado, sem erro vermelho (`Cancelled` não vira banner).
  - (e) Arquivo "Reunião ação 1.wav" funciona.
  - (f) Microfone: 30 s de fala → texto ao vivo → finalizar → passo 2.
- [ ] **Step 4:** Commit `feat(ui): passo de áudio e microfone`.

### Task 13: Passos ② Transcrição e ③ Resumo

**Files:**
- Create: `ui/js/views/step-transcript.js`, `ui/js/views/step-summary.js`
- Delete: `ui/spike-mic.html`
- Modify: `ui/css/app.css`

**Comportamento:**

- **Passo ② Transcrição**
  - Cabeçalho com `sourceName · modelo · idioma`. Omite o que for nulo.
  - Textarea com altura fixa (`calc(100vh - <cabeçalho>)`) e rolagem própria. O evento `input` marca `edited: true` e atualiza `transcript.text`.
  - `<details>` "Segmentos (N)" com horário e texto, fechado por padrão. Fica oculto se `segments` estiver vazio.
  - Botões Copiar (com feedback "Copiado ✓" por 1,4 s), TXT e SRT. SRT fica desabilitado sem segmentos.
  - "← Voltar" e "Resumir →". Resumir faz `goTo` 3 e põe `maxStep` em 3.
- **Passo ③ Resumo**
  - **Seletor de provedor:**
    - Vem de `providersStatus`.
    - Os provedores com `configured` aparecem selecionáveis. Os demais aparecem desabilitados com a observação "configurar" e um link para Configurações → Contas de IA.
    - `ClaudeCode` com `enabled: false` não aparece.
    - O valor inicial é `settings.summary_provider`, se ele estiver configurado. Senão, o primeiro configurado.
  - **Formato:** os 7 rótulos atuais (Executivo, Detalhado, Acadêmico, Reunião / ata, Aula / estudo, Podcast, Personalizado).
  - **Instruções e modelo:**
    - O textarea de instruções aparece só quando o formato é Personalizado.
    - O campo modelo é opcional, com placeholder = modelo padrão do provedor, ou "padrão da sua conta" para Codex e Claude Code.
    - Para Ollama, o placeholder lista os modelos instalados.
  - **Gerar:** respeita `startJob`/`canStartJob`. O resultado aparece num cartão com o HTML de `renderMarkdown`, o modelo, a quantidade de blocos ("3 blocos processados" quando for maior que 1), e os botões Copiar e "Salvar .md".
  - **Comparar:** checkboxes com os provedores configurados, de 2 a 4. O botão fica desabilitado fora desse intervalo. Os cartões aparecem lado a lado (`grid auto-fit minmax(280px,1fr)`), e o erro de um provedor aparece no cartão dele.
  - Ao gerar, grava `summary_provider` e `summary_style` em `setSettings`.
  - Voltar ao passo 2 e editar não apaga os resumos; uma nova transcrição apaga (via `afterTranscribe`).

- [ ] **Step 1:** Implemente.
- [ ] **Step 2:** `npm run test:ui`. Esperado: PASS.
- [ ] **Step 3: Verificação manual.**
  - (a) Do passo 2, ir às Configurações durante um resumo e voltar: o resumo aparece ao terminar.
  - (b) Um resumo com Markdown (títulos e listas) aparece formatado.
  - (c) Um `<script>` no texto do resumo não executa.
  - (d) Comparar com 1 provedor marcado deixa o botão desabilitado.
  - (e) SRT baixado bate com os segmentos.
  - (f) Texto colado → passo 2 sem SRT → resumo funciona.
- [ ] **Step 4:** Commit `feat(ui): passos de transcrição e resumo`.

### Task 14: Página de Configurações

**Files:**
- Create: `ui/js/views/settings.js`, `ui/js/views/settings-transcription.js`, `ui/js/views/settings-accounts.js`, `ui/js/views/settings-ollama.js`, `ui/js/views/settings-about.js`
- Modify: `ui/css/app.css`

**Comportamento:**

- **Estrutura:** uma página inteira, com "← Voltar" (`closeSettings`), menu lateral (Transcrição, Contas de IA, Ollama, Sobre) e uma seção por vez. Links de outras telas podem abrir direto numa seção, por exemplo `openSettings(state, 'accounts')`. Para isso, acrescente em `flow.js` o campo `settingsSection` e um teste.
- **Transcrição**
  - Uma linha por modelo do `CATALOG` com: rótulo, tamanho (MB/GB, formatação pt-BR), estado e ações.
  - **Não instalado:** Baixar.
  - **Baixando:** barra de progresso (eventos `model://progress`) e Cancelar.
  - **Instalado:** "✓ instalado", um rádio "padrão" (`setDefaultModel`) e Remover (pede confirmação dentro da tela). Não é possível remover o modelo padrão se for o único instalado.
  - Falha (`model://done` com `ok: false`) mostra o erro na própria linha, com "Tentar de novo".
  - Botão "Importar modelo existente…" usa `pickModelFile` e depois `importModel`.
  - Seletor de idioma padrão.
- **Contas de IA:** três grupos com os títulos exatos **Pela sua assinatura**, **Por chave de API** e **Local**.
  - **ChatGPT:**
    - Estado "Conectado: <detail>" ou "Não conectado".
    - "Entrar com ChatGPT" chama `codexLogin` e mostra "Conclua o login no navegador…" enquanto espera.
    - "Sair" chama `codexLogout`.
    - Se o sidecar estiver ausente (`Missing`): "Codex embutido não encontrado — reinstale o app".
  - **Claude Code:**
    - Interruptor "Usar o Claude Code instalado neste computador", que grava `claude_code_enabled`.
    - Texto fixo: "Usa o Claude Code que você instalou e logou neste computador. O consumo conta no seu plano Claude. O app não faz login nem guarda credenciais do Claude."
    - Estado: não encontrado / não logado / conectado.
  - **OpenAI, Anthropic e Gemini:**
    - Estado: "Chave salva no keychain" ou "Sem chave".
    - Campo de senha com Salvar. Trocar mostra o campo de novo; Remover chama `deleteApiKey`. A chave nunca aparece depois de salva.
    - Texto fixo do grupo: "Chaves de API são cobradas por uso na conta do provedor."
    - Nomes dos keychains: "Keychain do macOS" e "Gerenciador de Credenciais do Windows", conforme `navigator.userAgent`.
- **Ollama**
  - Estado: rodando ou "Ollama não encontrado", com o link `https://ollama.com/download` aberto pelo opener.
  - Lista de modelos com Remover.
  - Campo "Baixar modelo" (ex.: `qwen3:8b`) com o progresso de `ollama://progress` e Cancelar.
- **Sobre**
  - Versão do app e versões de whisper, FFmpeg e Codex (de `getStatus().versions`).
  - Pasta de dados com o botão "Abrir pasta".

- [ ] **Step 1:** Escreva o teste `openSettings com seção preserva o resto e define settingsSection`.
- [ ] **Step 2:** `npm run test:ui`. Esperado: FAIL.
- [ ] **Step 3:** Implemente.
- [ ] **Step 4:** `npm run test:ui`. Esperado: PASS.
- [ ] **Step 5: Verificação manual.**
  - (a) Baixar Base, cancelar no meio e baixar de novo até o fim: aparece "✓ instalado".
  - (b) Desligar o Wi-Fi no meio de um download: a linha mostra o erro, o modelo não fica instalado e "Tentar de novo" funciona.
  - (c) Importar `ggml-large-v3-turbo.bin` do projeto antigo.
  - (d) Salvar uma chave falsa da OpenAI: o estado muda, e um resumo com ela mostra um erro 401 legível no passo 3.
  - (e) "Entrar com ChatGPT" abre o navegador, e depois do login o estado mostra "Conectado".
  - (f) Ligar o Claude Code: aparece "conectado" (neste Mac ele está logado).
  - (g) Sem o Ollama rodando, aparece "não encontrado".
- [ ] **Step 6:** Commit `feat(ui): página de configurações`.

## Fase D — Empacotamento

### Task 15: Sidecars (FFmpeg LGPL, Codex) e build local

**Files:**
- Create: `scripts/sidecars.lock.json`, `scripts/fetch-sidecars.sh`, `scripts/fetch-sidecars.ps1`, `scripts/build-ffmpeg-macos.sh`, `src-tauri/binaries/.gitkeep`
- Modify: `src-tauri/tauri.conf.json` (`bundle.externalBin: ["binaries/ffmpeg", "binaries/codex"]`), `src-tauri/src/state.rs` (lê as versões para `get_status`)

**Interfaces:**
- Produces: `scripts/sidecars.lock.json`, com `{ "codex": { "version": "...", "aarch64-apple-darwin": {"url": "...", "sha256": "..."}, "x86_64-pc-windows-msvc": {...} }, "ffmpeg": { "version": "...", "x86_64-pc-windows-msvc": {"url": "...", "sha256": "..."}, "aarch64-apple-darwin": {"source_url": "...", "sha256": "..."} } }`.
- Os scripts deixam `src-tauri/binaries/ffmpeg-<triple>[.exe]` e `codex-<triple>[.exe]`. Se o SHA-256 não bater, abortam com código ≠ 0.

**Origem de cada sidecar:**
- **Codex:** o binário da GitHub Release `openai/codex` da versão fixada, para cada triple.
- **FFmpeg no Windows:** o build LGPL do BtbN (`ffmpeg-n<ver>-latest-win64-lgpl-<ver>.zip`), de onde se extrai só o `ffmpeg.exe`.
- **FFmpeg no macOS:** não há build LGPL arm64 confiável pronto, então o `build-ffmpeg-macos.sh` compila a partir do tarball oficial fixado. A compilação deve:
  - usar `--disable-gpl --disable-nonfree --disable-everything --disable-network --disable-doc --disable-ffplay --disable-ffprobe --enable-static --disable-shared`;
  - habilitar os demuxers `mov,mp3,wav,ogg,matroska,webm_dash_manifest,flac,aac`;
  - habilitar os decoders `aac,mp3,mp3float,opus,vorbis,flac,pcm_s16le,pcm_s24le,pcm_f32le,alac`;
  - habilitar os parsers `aac,mpegaudio,opus,vorbis,flac`;
  - habilitar o protocolo `file,pipe`, o filtro `aresample`, o encoder `pcm_f32le` e o muxer `f32le`;
  - habilitar `swresample`.
  - Confirme se `ffmpeg -version` não mostra `--enable-gpl`.

- [ ] **Step 1:** Escreva o `scripts/verify-sidecars.sh`: falha se um sidecar estiver ausente, se `ffmpeg -version` contiver `enable-gpl`, ou se `codex --version` não bater com o lock. Rode-o. Esperado: FAIL (nada baixado ainda).
- [ ] **Step 2:** Implemente os scripts e o lock, com URLs e SHA-256 reais calculados agora.
- [ ] **Step 3:** No Mac, rode `scripts/fetch-sidecars.sh && scripts/verify-sidecars.sh`. Esperado: PASS.
- [ ] **Step 4:** Rode `npm run tauri build`. Esperado: são gerados `src-tauri/target/release/bundle/macos/Local Transcriber.app` e o `.dmg`.
- [ ] **Step 5: Verificação manual.**
  - Mova o `.app` para `/Applications` e abra pelo Finder, com um PATH mínimo.
  - Esperado: transcrição de `jfk.wav` funciona sem Homebrew no PATH.
  - Para conferir: `env -i /Applications/Local\ Transcriber.app/Contents/MacOS/local-transcriber` também abre.
  - "Entrar com ChatGPT" usa o Codex embutido; confira a versão na tela Sobre.
- [ ] **Step 6:** Commit `build: sidecars ffmpeg lgpl e codex`.

### Task 16: GitHub Actions — build Mac/Windows e Release

**Files:**
- Create: `.github/workflows/build.yml`

**Comportamento:**
- **Gatilhos:** `push` em qualquer branch, `pull_request`, e `push` de tags `v*`.
- **Matriz:**
  - `macos-14` com target `aarch64-apple-darwin`, rodando `fetch-sidecars.sh`;
  - `windows-latest` com target `x86_64-pc-windows-msvc`, rodando `fetch-sidecars.ps1`.
- **Passos de cada job:** checkout → Rust stable → Node LTS → `npm ci` → script de sidecars → `verify-sidecars` → `cargo test` (em `src-tauri`) → `npm run test:ui` → `npm run tauri build`.
  - Use `swatinem/rust-cache` e cacheie o FFmpeg compilado no Mac pela chave do `sidecars.lock.json`.
- **Saída:**
  - Em branch ou PR, os artefatos sobem via `actions/upload-artifact`: o `.dmg` e o `.msi`/`-setup.exe`.
  - Numa tag `v*`, um job final cria a GitHub Release com os instaladores, via `softprops/action-gh-release` numa versão fixada.
- **Ações de terceiros:** todas com a versão fixada por SHA de commit.
- **Permissões:** `contents: write` só no job de release.

- [ ] **Step 1:** Escreva o workflow.
- [ ] **Step 2:** Faça push de uma branch `ci/build`. Esperado: os dois jobs ficam verdes e os artefatos `.dmg` e `.msi` aparecem no run.
- [ ] **Step 3: Verificação manual no Windows** (pendente da Task 5, se houver): instale o `.msi`, passe pelo SmartScreen ("Mais informações → Executar assim mesmo") e transcreva `jfk.wav` com o modelo Base. O microfone funciona.
- [ ] **Step 4:** Commit `ci: build mac e windows com release por tag`. Abra o PR para `main`.

### Task 17: Remover o app Python e reescrever o README

**Files:**
- Delete: `app.py`, `ai_engine.py`, `static/`, `install.sh`, `start.sh`, `requirements.txt`
- Modify: `README.md`

**Antes de apagar:** confira com `grep` que os textos literais (`SYSTEM`, os 7 prompts de formato e os prompts de trecho/consolidação) estão idênticos em `src-tauri/src/summarize.rs`. Para isso, escreva o teste `prompts_match_python_source`, que só roda enquanto `ai_engine.py` existe (`#[ignore]`). Rode-o e só então apague.

**Conteúdo do README (em português):**
- o que é o app;
- como baixar da Release, incluindo como passar pelo Gatekeeper no Mac (botão direito → Abrir, ou Ajustes → Privacidade e Segurança → Abrir mesmo assim) e pelo SmartScreen no Windows;
- primeiro uso: baixar um modelo em Configurações;
- contas de IA:
  - ChatGPT pelo login embutido;
  - chaves de API com aviso de cobrança;
  - Claude Code opcional, com o aviso de que usa o plano da própria pessoa;
  - Ollama;
- privacidade: só sai do computador o que você manda resumir na nuvem;
- como gerar localmente: pré-requisitos por SO (Mac: Xcode CLT, Rust, Node, CMake; Windows: VS Build Tools, Rust, Node, CMake, LLVM), seguidos de `npm ci`, `scripts/fetch-sidecars.*` e `npm run tauri build`;
- licenças de terceiros: whisper.cpp (MIT), FFmpeg (LGPL, com link para o código-fonte da versão usada), Codex CLI (Apache-2.0), marked (MIT), DOMPurify (Apache-2.0/MPL).

- [ ] **Step 1:** Rode `cargo test prompts_match_python_source -- --ignored`. Esperado: PASS.
- [ ] **Step 2:** Apague os arquivos e reescreva o README. Remova também o teste ignorado, que perde o sentido sem `ai_engine.py`.
- [ ] **Step 3:** `cargo test && npm run test:ui && npm run tauri build`. Esperado: PASS e build gerado.
- [ ] **Step 4:** Commit `chore: remove app python e atualiza README`. Faça push e confira que o CI fica verde.
