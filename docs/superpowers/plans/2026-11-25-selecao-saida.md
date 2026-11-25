# Escolha da saída de som — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Na aba Gravar, escolher qual saída de som é gravada como "som do computador", com volta automática para a primeira opção se ela cair.

**Architecture:** O backend lista as saídas (cpal) e abre o som do computador preso a uma saída: tap do Core Audio por dispositivo no Mac, loopback WASAPI no Windows. A captura informa a que está presa (`OutputBinding`); a sessão confere a cada 1 s e reabre quando a saída padrão mudou (comportamento atual) ou quando a saída escolhida sumiu da lista. O fechamento de reabertura em `commands/recording.rs` decide cair para a primeira opção e manda um aviso (`record://notice`) que a interface mostra sob as barras.

**Tech Stack:** Rust (Tauri v2.12.7, cpal =0.18.2, objc2-core-audio =0.3.2), JS ES modules sem framework, `node --test`.

**Spec:** `docs/superpowers/specs/2026-11-25-selecao-saida-design.md`

## Global Constraints

- Sem dependências novas; versões fixadas como estão.
- Mac: som do computador sempre por process tap do Core Audio (nunca cpal). Saída escolhida = `CATapDescription::initExcludingProcesses_andDeviceUID_withStream(alloc, &NSArray::new(), &uid, 0)` com mute `Unmuted` e `private` como hoje. Sem saída escolhida = o tap global atual, intocado.
- O id de saída é o `cpal::DeviceId` em texto (`coreaudio:<UID>` no Mac, `wasapi:<id>` no Windows). O UID para o tap é `DeviceId::id()` (a parte depois de `host:`), obtido via `DeviceId::from_str`.
- Textos exatos (pt-BR):
  - Rótulo do seletor: `Saída de som`
  - Primeira opção: `Todas as saídas` (Mac) / `Saída padrão` (Windows); valor vazio (`""` → `null`).
  - Saída padrão do sistema na lista: `<nome> (padrão)`.
  - Aviso de queda: `A saída de som escolhida foi desconectada. Gravando agora de todas as saídas.` (Mac) / `A saída de som escolhida foi desconectada. Gravando agora da saída padrão.` (Windows).
  - Botão recarregar: `title`/`aria-label` `Recarregar saídas de som`.
  - Silêncio com saída escolhida: `Nada do som do computador até agora. Confira se a reunião está tocando na saída escolhida.`
  - `sourceName`: `Som do computador — <nome>` quando há saída escolhida no modo Som do computador; os demais como hoje.
- Configuração nova: `record_output_id: Option<String>`, leniente (valor inválido → `None`), padrão `None`. Só muda quando a fonte usada inclui o som do computador.
- Saída salva ausente no começo → primeira opção, sem aviso. Depois de cair para a primeira opção, a sessão fica nela até o fim (não volta sozinha).
- Commits: mensagens simples, SEM Co-Authored-By, SEM menção ao Claude, SEM link de sessão. Não tocar `models/`, `outputs/`, `LICENSE`.

## Review Focus

1. Saída escolhida desconectada no meio (fone Bluetooth/USB): a gravação continua na primeira opção, um aviso só, sem loop de reaberturas. → testes de sessão na Task 2.
2. Listagem de saídas falha ou volta vazia por um instante: não deve derrubar a captura escolhida. → teste `pinned_listing_error_keeps_capture` na Task 2.
3. Saída salva que não existe mais ao abrir o app: o seletor mostra a primeira opção e a gravação usa a primeira opção. → testes `pickOutputId` (Task 3) e `resolve_output` (Task 1).
4. Modo "Microfone" puro: nada de saída é salvo nem enviado; trocar para Microfone não apaga a saída salva. → teste `remember_choice` (Task 2) e teste do view helper (Task 3).
5. Mac com saída escolhida que só tem entrada ou zero streams de saída: erro claro em pt-BR, não pânico. → a lista só traz dispositivos de saída (Task 1); falha do tap vira `AppError` com o texto atual de `check(...)`.

---

### Task 1: Listar saídas e abrir o som do computador preso a uma saída

**Files:**
- Modify: `src-tauri/src/record/devices.rs`
- Modify: `src-tauri/src/record/mac_tap.rs`
- Test: os módulos de teste dos mesmos arquivos

**Interfaces:**
- Produces:
  - `pub fn list_outputs() -> Result<Vec<AudioInput>, AppError>` (mesma struct `AudioInput { id, name, is_default }`; padrão primeiro). Erro: `"Não foi possível listar as saídas de som: {e}"`.
  - `pub fn resolve_output<'a>(outputs: &'a [AudioInput], wanted: Option<&str>) -> Option<&'a AudioInput>` — só a pedida, se existir; senão `None` (= primeira opção). Diferente de `resolve_mic`: NÃO cai para a padrão.
  - `pub fn output_ids() -> Option<Vec<String>>` — ids das saídas presentes; `None` se a listagem falhar.
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub enum OutputBinding { None, FollowDefault(String), Pinned(String) }`.
  - `OpenSource::binding(&self) -> &OutputBinding` substitui `output_id()`. Microfone e tap global → `None`; loopback padrão do Windows → `FollowDefault(id)`; saída escolhida (Mac e Windows) → `Pinned(id)`.
  - `pub fn open(kind: SourceKind, device_id: Option<&str>, tx: SyncSender<SourceMsg>) -> Result<OpenSource, AppError>` — `device_id` é o microfone para `Mic` e a saída escolhida para `System` (`None` = primeira opção).
  - `pub fn device_uid(id: &str) -> Option<String>` — `DeviceId::from_str(id).ok().map(|d| d.id().to_string())` (usado pelo tap; testável sem dispositivo).
  - Mac: `SystemTap::open(sink, fault, device_uid: Option<&str>)`.
  - `device_name()` de uma saída escolhida = nome do dispositivo; da primeira opção = `"Som do computador"` (Mac) ou o nome da saída padrão (Windows, como hoje).

- [ ] **Step 1: Testes que falham** (em `devices.rs`)
  - `resolve_output_finds_wanted`: lista `[a(default), b]`, `Some("b")` → `b`.
  - `resolve_output_missing_is_none`: `Some("z")` → `None`; `None` → `None` (não cai para a padrão).
  - `device_uid_strips_host`: `device_uid("coreaudio:BuiltInSpeakerDevice")` → `Some("BuiltInSpeakerDevice")` (no Mac; no Windows use `"wasapi:{abc}"` → `Some("{abc}")`; escreva o teste com `#[cfg]` por plataforma); `device_uid("semhost")` → `None`.
  - Ajuste os testes que usavam `output_id()` para `binding()`.
- [ ] **Step 2:** `cd src-tauri && cargo test record::devices` → falham por não compilar/assert.
- [ ] **Step 3: Implementar** `list_outputs` (mesmo padrão de `list_inputs`, sobre `output_devices()`/`default_output_device()`), `resolve_output`, `output_ids`, `device_uid`, `OutputBinding`, o parâmetro novo de `open`:
  - Mac `System` + `Some(id)`: `device_uid(id)` → `SystemTap::open(sink, fault, Some(&uid))`; nome do dispositivo pela lista/`device_by_id`; `binding = Pinned(id)`. `id` inválido → `AppError::User("Saída de som não encontrada.")`.
  - Windows `System` + `Some(id)`: `host.device_by_id(&DeviceId::from_str(id)?)` → loopback como hoje sobre esse dispositivo; `binding = Pinned(id)`. Não encontrado → mesmo erro.
  - `System` + `None`: exatamente o comportamento atual (Mac tap global `binding None`; Windows saída padrão `FollowDefault(id)`).
  - `mac_tap.rs`: com `device_uid` `Some`, a descrição vem de `initExcludingProcesses_andDeviceUID_withStream(..., 0)`; o resto (mute, private, agregado, listeners, teardown) igual. Comentário curto dizendo que a stream 0 é a gravada.
- [ ] **Step 4:** `cargo test` → tudo passa, sem warnings.
- [ ] **Step 5: Teste manual real (Mac, esta máquina)** — adicione ao teste ignorado que já abre dispositivos reais (o que imprime `"{kind:?}: aberto ..."`) um caso que abre `System` preso à saída padrão (`list_outputs()` → a `is_default`) e confirma que abre e recebe pacotes enquanto algo toca (`afplay /System/Library/Sounds/Glass.aiff` em loop numa thread). Rode com `cargo test -- --ignored <nome> --nocapture` e cole a saída no relatório.
- [ ] **Step 6: Commit** `feat: listar saídas de som e gravar uma saída escolhida`

### Task 2: Sessão segue a saída escolhida; comando e configuração

**Files:**
- Modify: `src-tauri/src/record/session.rs`, `src-tauri/src/commands/recording.rs`, `src-tauri/src/settings.rs`, `src-tauri/src/lib.rs`
- Test: módulos de teste desses arquivos

**Interfaces:**
- Consumes: Task 1 (`OutputBinding`, `binding()`, `open(kind, device_id, tx)`, `list_outputs`, `resolve_output`, `output_ids`).
- Produces:
  - `trait Capture { fn binding(&self) -> &OutputBinding; fn dropped(&self) -> u64; }` (substitui `output_id`).
  - `Env { now, default_output, outputs: Arc<dyn Fn() -> Option<Vec<String>> + Send + Sync> }`; `Env::system()` usa `devices::output_ids`.
  - `pub struct Opened { capture, rx, notice: Option<String> }`.
  - `SessionEvent::Notice { message: String }` → evento `record://notice` com `{ message }`.
  - `devices::should_reopen` passa a receber o vínculo: `pub fn should_reopen(binding: &OutputBinding, default_now: Option<&str>, outputs_now: Option<&[String]>) -> bool` — `FollowDefault(id)`: como hoje; `Pinned(id)`: `outputs_now` é `Some(l)` e `l` não contém `id`; `None`: false. Listagem `None` nunca reabre um `Pinned`.
  - Comando `list_audio_outputs() -> Result<Vec<AudioInput>, AppError>` registrado em `lib.rs`.
  - `start_recording(source, mic_id, output_id, model, language)` (JS: `outputId`).
  - `Settings.record_output_id: Option<String>` leniente.
  - Constante do aviso (por plataforma, textos das Global Constraints) em `devices.rs`: `pub(super) const OUTPUT_FALLBACK: &str`.

- [ ] **Step 1: Testes que falham**
  - `devices.rs`: `should_reopen` — `Pinned("a")` com `Some(["a","b"])` → false; com `Some(["b"])` → true; com `None` → false; `FollowDefault("x")` com padrão `Some("y")` → true (mantém os 3 testes atuais adaptados); `None` → false sempre.
  - `session.rs` (com as fakes existentes; a fake ganha `binding` e o `Env` de teste ganha `outputs` controlável):
    - `pinned_output_gone_reopens_with_notice`: captura `Pinned("a")`, `outputs` passa a `Some(["b"])`; o `reopen` devolve `Opened { notice: Some(OUTPUT_FALLBACK) , binding None }` → exatamente um `SessionEvent::Notice` com esse texto; a sessão continua (sem `Error`); `tries` não sobe (troca bem-sucedida não conta).
    - `pinned_listing_error_keeps_capture`: `outputs` devolve `None` por vários ciclos → `reopen` nunca é chamado.
    - `notice_on_failed_reopen_path`: a captura `Pinned` manda `Failed(...)`; o `reopen` devolve `Opened` com `notice` → um `Notice` e a sessão segue.
  - `recording.rs`: `remember_choice` — fonte `"system"` grava `record_output_id`; `"both"` grava os dois; `"mic"` não mexe em `record_output_id`. `event_payload(Notice)` → `("record://notice", {"message": ...})`. `Mode::System.source_name` com nome de saída → `"Som do computador — Fones"`; sem → `"Som do computador"`.
  - `settings.rs`: `record_output_id` padrão `None`; valor inválido (número) → `None` sem perder os outros campos.
- [ ] **Step 2:** `cargo test` → falham.
- [ ] **Step 3: Implementar**
  - `Live::maintain`: a checagem de 1 s usa `devices::should_reopen(capture.binding(), default_now, outputs_now)`; chame `(env.default_output)()` só para `FollowDefault` e `(env.outputs)()` só para `Pinned`. `install` emite `SessionEvent::Notice` quando `opened.notice` é `Some` (o `Live` precisa do `Shared`/`sink`: passe-o como argumento de `maintain`/`on_failed` ou guarde um `Sink` no `Live` — escolha um e comente).
  - `begin`: para o som do computador, `outputs = devices::list_outputs()?`; `chosen = resolve_output(&outputs, output_id)` → `Some(id)` ou `None` (primeira opção, sem aviso). O `reopen` do som do computador guarda o id escolhido num estado mutável dentro do fechamento: a cada chamada, se o id escolhido ainda está em `output_ids()` (ou a listagem falhou), reabre preso a ele (`notice: None`); se sumiu, abre a primeira opção, zera o id guardado e devolve `notice: Some(OUTPUT_FALLBACK.into())`. O microfone continua como hoje (`notice: None`).
  - `start_recording` recebe `output_id`, salva com `save_choice` (assinatura ganha `output_id`), passa adiante; `list_audio_outputs` igual a `list_audio_inputs` (`spawn_blocking`), mensagem de falha `"Falha ao listar as saídas de som: {e}"`.
- [ ] **Step 4:** `cargo test` → tudo passa, zero warnings.
- [ ] **Step 5: Commit** `feat: sessão segue a saída escolhida e cai para a primeira opção`

### Task 3: Seletor "Saída de som" e aviso na interface

**Files:**
- Modify: `ui/js/api.js`, `ui/js/recorder.js`, `ui/js/views/step-audio.js`, `ui/css/app.css` (só se precisar), `README.md` (seção Gravar: uma frase sobre escolher a saída e a queda para a primeira opção)
- Test: `ui/tests/recorder.test.js`

**Interfaces:**
- Consumes: Task 2 (`list_audio_outputs`, `start_recording(..., outputId, ...)`, `record://notice {message}`, `settings.record_output_id`).
- Produces (em `recorder.js`):
  - `export const showOutputPicker = (source) => source !== 'mic';`
  - `export function pickOutputId(outputs, savedId)` → `savedId` se existir na lista; senão `null`.
  - `export function firstOutputLabel(mac)` → `'Todas as saídas'` / `'Saída padrão'`.
  - `export function outputOptions(outputs, mac)` → `[{ value: '', text: firstOutputLabel(mac) }, ...outputs.map(o => ({ value: o.id, text: o.is_default ? `${o.name} (padrão)` : o.name }))]`.
  - `export const PINNED_SILENCE_HINT` (texto exato das Global Constraints) e `showSilenceHint({ mac, source, peak, elapsedMs, pinned })` → mostra quando usa o som do computador, está em silêncio e (`mac` ou `pinned`); `silenceHintText({ pinned })` → `PINNED_SILENCE_HINT` se `pinned`, senão `SILENCE_HINT`.
  - `createRecorder({ onText, onLevel, onError, onNotice })` escuta `record://notice` (só com `active`) e chama `onNotice(message)`; `start({ source, micId, outputId, model, language })`.
  - `api.listAudioOutputs()`, `api.startRecording(source, micId, outputId, model, language)`.

- [ ] **Step 1: Testes que falham** (`ui/tests/recorder.test.js`)
  - `pickOutputId`: salvo presente → ele; salvo ausente → `null`; `undefined` → `null`; lista vazia → `null`.
  - `outputOptions`: Mac → primeira `{value:'', text:'Todas as saídas'}`; Windows → `'Saída padrão'`; padrão marcada `'Fones (padrão)'`.
  - `showOutputPicker`: `'mic'` false; `'system'`/`'both'` true.
  - `showSilenceHint`: Windows + `pinned` + silêncio 12 s → true; Windows sem `pinned` → false; Mac sem `pinned` → true (como hoje). `silenceHintText`.
  - `createRecorder`: com a fake de `on`, um `record://notice` durante a gravação chama `onNotice('...')`; depois de `stop` é ignorado; `start` repassa `outputId` para `startRecording` na 3ª posição.
- [ ] **Step 2:** `npm run test:ui` → falham.
- [ ] **Step 3: Implementar** o seletor em `step-audio.js`: linha `output-row` igual à `mic-row` (label `Saída de som`, `select#output-sel`, botão ↻ com `title`/`aria-label` `Recarregar saídas de som`), entre `micRow` e `hint`; escondida quando `!showOutputPicker(source)`. Carrega as saídas junto com os microfones (mesmo momento de `loadInputs`, chamada separada; erro na listagem → seletor só com a primeira opção). `ui.outputId = pickOutputId(...)` com `store.get().settings?.record_output_id`. Ao começar: `outputId: showOutputPicker(source) ? ui.outputId : null`; o `store.set` de settings grava `record_output_id` só quando `showOutputPicker(source)`. Aviso: um `div.hint` (`outputNote`) sob `silenceNote`, `aria-live="polite"`, mostra a mensagem do `onNotice` até o fim da gravação e é limpo ao começar a próxima. O silêncio usa `pinned: !!ui.outputId` e o texto de `silenceHintText`.
- [ ] **Step 4:** `npm run test:ui` → tudo passa.
- [ ] **Step 5: Build e verificação** `npm run tauri build`; abrir o `.app` gerado ~5 s (deve subir sem erro). Liste no relatório os testes manuais: escolher uma saída não padrão e tocar som nela; tocar som em outra saída (não deve aparecer); desconectar o fone escolhido no meio (aviso + continua).
- [ ] **Step 6: Commit** `feat: seletor da saída de som na gravação`
