# Captura de áudio (microfone escolhido + som do computador) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Gravar do microfone escolhido, do som do computador ou dos dois. No modo duplo, a transcrição é etiquetada "Você"/"Reunião" e intercalada por tempo. Tudo nativo em Rust, com um indicador visual de gravação.

**Architecture:**
- A captura sai da webview (`MediaRecorder` + `transcribe_chunk`) e passa para Rust com `cpal` 0.18.2.
- Microfone: stream de entrada no dispositivo escolhido. Som do computador: stream de entrada aberto sobre o dispositivo de saída padrão. A `cpal` faz loopback por process tap no macOS 14.6+ e por WASAPI no Windows.
- Cada fonte vira PCM mono 16 kHz num `Track`. O `Track` tem um relógio comum: completa lacunas com zeros pelo tempo real e corta blocos de 12 s.
- Os blocos vão para uma fila única de transcrição (um Whisper por vez). Os segmentos são deslocados pelo índice do bloco e, no modo duplo, etiquetados e intercalados.
- A interface recebe `record://text`, `record://level` e `record://error`.

**Tech Stack:** Rust (Tauri 2.12.7, cpal 0.18.2, rubato 5.1.0, whisper-rs 0.16.0), JS vanilla ES modules, `node --test`.

**Spec:** `docs/superpowers/specs/2026-11-24-captura-audio-design.md`

## Global Constraints

- **Versões:** dependências novas com versão exata (`=x.y.z`): `cpal = "=0.18.2"`, `rubato = "=5.1.0"`.
- **macOS:** `bundle.macOS.minimumSystemVersion` passa a `"14.6"`. O `Info.plist` ganha `NSAudioCaptureUsageDescription` = "O Local Transcriber grava o som do computador para transcrever reuniões no seu computador."
- **Etiquetas:** internas `"voce"` / `"reuniao"`; exibidas `Você` / `Reunião`.
- **Texto do modo duplo:** uma linha por grupo, `[mm:ss] Você: texto`. A partir de 1 h, `[h:mm:ss]`. Segmentos seguidos da mesma fonte se juntam na mesma linha, com o tempo do primeiro. Linhas separadas por `\n`.
- **Outros modos** (arquivo, só microfone, só som do computador): texto como hoje (`join_text`), `speaker: null`.
- **`sourceName`:** `Microfone — <nome do dispositivo>`, `Som do computador`, `Microfone + som do computador`.
- **Tempos:** blocos de 12 s (`CHUNK_SECONDS = 12`), 16 kHz mono f32. O relógio comum completa com zeros quando a fonte fica mais de 200 ms atrás do tempo real. O evento `record://level` sai a cada 100 ms.
- **Callbacks:** o callback de áudio da `cpal` nunca bloqueia (sem `Mutex::lock`, sem alocação grande, sem I/O). Ele envia por canal com `try_send`; se o canal estiver cheio, descarta e conta.
- **Exclusividade:** a gravação ocupa o mesmo slot `transcription_cancel` que `transcribe_file` ("Já existe uma transcrição em andamento.").
- **Texto da interface:** em pt-BR. O aviso de fone é exatamente "Use fone de ouvido para a fala dos outros não ser gravada duas vezes."
- **Commits:** só a mensagem. SEM `Co-Authored-By`, sem mencionar Claude, sem link de sessão.
- **Suíte limpa:** `cd src-tauri && cargo test` sem warnings; `npm run test:ui` passando.

## Review Focus

1. **Saída padrão trocada no meio da gravação** (fone conectado depois). Esperado: "Reunião" continua sendo gravada pelo novo dispositivo, sem erro. O teste da Task 3 cobre a decisão de reabrir com um fake, e a verificação real é manual.
2. **Som do computador sem nada tocando por minutos** (o loopback do Windows não entrega amostras). Esperado: os tempos de "Reunião" não se adiantam em relação a "Você". Teste de `Track` na Task 2.
3. **Microfone escolhido desconectado no meio da gravação.** Esperado: a gravação para com uma mensagem e o que já foi transcrito vai para o passo ②. Teste de sessão na Task 4 com fonte fake que falha.
4. **Transcrição mais lenta que o tempo real** (modelo grande, máquina lenta). Esperado: a fila cresce, nada é perdido, e "Parar" espera esvaziar. Teste de fila na Task 4.
5. **Microfone salvo que não existe mais** (outro computador, fone trocado). Esperado: o padrão é usado sem erro. Teste na Task 3.

---

### Task 1: Base: dependências, macOS 14.6, `Segment.speaker`, configurações

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/Info.plist`, `src-tauri/src/transcribe.rs`, `src-tauri/src/settings.rs`, `README.md` (requisito de macOS 14.6)

**Interfaces:**
- Produces:
  - `transcribe::Segment { start, end, text, speaker: Option<String> }`, serializado como `speaker: null | "voce" | "reuniao"`.
  - `Engine::transcribe` continua devolvendo segmentos com `speaker: None`.
  - `Settings.record_source: String` (`"mic"` | `"system"` | `"both"`, padrão `"mic"`, leitura tolerante: valor inválido vira `"mic"`) e `Settings.record_mic_id: Option<String>` (padrão `None`, tolerante).

- [ ] **Step 1: Testes que falham** em `settings.rs`:
  - `record_fields_default` (`{}` → `"mic"`, `None`);
  - `record_source_invalid_becomes_mic` (`{"record_source":"xyz"}` → `"mic"`);
  - `record_mic_id_wrong_type_is_none` (`{"record_mic_id":5}` → `None`, e os outros campos preservados).

  Em `transcribe.rs`: `segment_serializes_speaker_null`. Um `Segment` sem etiqueta serializa com `"speaker":null`.
- [ ] **Step 2:** `cargo test settings transcribe` → FAIL (campos não existem).
- [ ] **Step 3:** Implementar:
  - deps `cpal = "=0.18.2"` e `rubato = "=5.1.0"`;
  - `minimumSystemVersion` `"14.6"`;
  - `NSAudioCaptureUsageDescription` com o texto das Global Constraints;
  - campo `speaker` e campos de `Settings` com desserializador tolerante igual ao `lenient_provider`.

  Ajustar no código existente os lugares que constroem `Segment`. Os testes que comparam `Segment` recebem `speaker: None`. README: requisito "macOS 14.6 ou mais novo".
- [ ] **Step 4:** `cargo test` → PASS, sem warnings. `npm run test:ui` → PASS.
- [ ] **Step 5:** Commit `feat: base da captura nativa (cpal, macOS 14.6, speaker, preferências)`.

### Task 2: Lógica pura da gravação: reamostragem, relógio/blocos, nível, etiquetas

**Files:**
- Create: `src-tauri/src/record/mod.rs`, `src-tauri/src/record/resample.rs`, `src-tauri/src/record/track.rs`, `src-tauri/src/record/merge.rs`
- Modify: `src-tauri/src/lib.rs` (`mod record;`)

**Interfaces:**
- Produces:
  - `resample::ToMono16k::new(in_rate: u32, channels: u16) -> Result<Self, AppError>` e `fn push(&mut self, interleaved: &[f32]) -> Vec<f32>`.
    - Faz downmix pela média dos canais e reamostra para 16 kHz com rubato.
    - `push` aceita tamanhos arbitrários e guarda internamente o resto que ainda não fechou um bloco do rubato.
    - `fn flush(&mut self) -> Vec<f32>` entrega o que sobrou.
  - `track::Track::new(start: Instant)`:
    - `fn push(&mut self, pcm16k: &[f32], now: Instant)`. Antes de anexar, se `samples_total < (now - start) em amostras - 200 ms`, anexa zeros até alcançar.
    - `fn pad_to(&mut self, now: Instant)` aplica a mesma regra sem dados novos; chamado periodicamente para a fonte calada.
    - `fn take_chunk(&mut self) -> Option<(u32 /*índice*/, Vec<f32>)>` devolve blocos completos de `12 * 16000` amostras.
    - `fn finish(self) -> Option<(u32, Vec<f32>)>` devolve o bloco parcial final, se tiver ao menos 0,5 s.
  - `track::level(pcm: &[f32]) -> f32`: RMS em dBFS mapeado linearmente de −60..0 para 0..1, com clamp.
  - `merge::shift_ts(ts: &str, offset_ms: i64) -> String`: mesmo formato `HH:MM:SS,mmm` de `format_ts`; entrada inválida volta igual.
  - `merge::shift_segments(segs: Vec<Segment>, chunk_index: u32, speaker: Option<&str>) -> Vec<Segment>`: offset = índice × 12 000 ms.
  - `merge::interleave(a: Vec<Segment>, b: Vec<Segment>) -> Vec<Segment>`: ordena por `start`; empate mantém `a` (Você) antes.
  - `merge::labeled_text(segs: &[Segment]) -> String`: formato das Global Constraints.

- [ ] **Step 1: Testes que falham.**
  - `resample`:
    - 48 kHz estéreo, 1 s de seno de 440 Hz → 16000 ± 1 % amostras, mono, com pico de frequência preservado (contagem de cruzamentos por zero ≈ 880 ± 2 %);
    - 44,1 kHz mono → 16000 ± 1 %;
    - entradas em pedaços de 1, 7 e 1000 amostras somam o mesmo total que de uma vez (± 1 %).
  - `track`:
    - 30 s de dados contínuos → 2 blocos de 192 000 (índices 0 e 1) + `finish` com 6 s;
    - fonte calada: `push` de 1 s, depois `pad_to(start+20s)` → o bloco 0 existe e as amostras 16000..192000 são zero;
    - jitter: dados 150 ms atrasados não geram padding;
    - `finish` com 0,3 s → `None`.
  - `level`: silêncio → 0.0; seno com amplitude 1.0 → ≈ 0.95..1.0; −30 dBFS → ≈ 0,5 ± 0,05.
  - `merge`:
    - `shift_ts("00:00:05,000", 12_000) == "00:00:17,000"`;
    - virada de hora: `shift_ts("00:59:59,500", 1_000) == "01:00:00,500"`;
    - inválido inalterado;
    - `shift_segments` aplica o índice 2 → +24 s e define `speaker`;
    - `interleave`: a ordem por tempo e o empate deixam Você primeiro;
    - `labeled_text`: junta Você+Você na mesma linha; resultado `"[00:12] Você: Bom dia. Vamos.\n[00:15] Reunião: Oi."`;
    - acima de 1 h: `"[1:02:03] Você: …"`;
    - texto vazio ignorado.
- [ ] **Step 2:** `cargo test record::` → FAIL (stubs com `todo!()`).
- [ ] **Step 3:** Implementar. Usar a API do rubato 5.1.0 com razão fixa (verificar a API real na fonte da crate em `~/.cargo/registry`, porque ela mudou entre versões).
- [ ] **Step 4:** `cargo test` → PASS, sem warnings.
- [ ] **Step 5:** Commit `feat: lógica pura da gravação (reamostragem, relógio comum, etiquetas)`.

### Task 3: Dispositivos com cpal: lista, microfone, som do computador e troca de saída

**Files:**
- Create: `src-tauri/src/record/devices.rs`
- Modify: `src-tauri/src/record/mod.rs`

**Interfaces:**
- Consumes: `resample::ToMono16k` (Task 2).
- Produces:
  - `#[derive(Serialize)] AudioInput { id: String, name: String, is_default: bool }`.
    - `id` é o `cpal::DeviceId` em texto (`Display`/`FromStr` da cpal). `name` vem de `description()`.
  - `devices::list_inputs() -> Result<Vec<AudioInput>, AppError>`: o padrão vem primeiro.
  - `devices::resolve_mic<'a>(inputs: &'a [AudioInput], wanted: Option<&str>) -> Option<&'a AudioInput>`: função pura. Devolve o `wanted` se existir; senão o padrão; senão o primeiro; senão `None`.
  - `enum SourceKind { Mic, System }`.
  - `devices::open(kind: SourceKind, mic_id: Option<&str>, tx: std::sync::mpsc::SyncSender<SourceMsg>) -> Result<OpenSource, AppError>`:
    - `SourceMsg::Pcm(Vec<f32>)`: já em mono 16 kHz. A conversão roda numa thread do `OpenSource`, não no callback.
    - `SourceMsg::Failed(String)`.
    - O callback da cpal faz só `try_send` das amostras brutas para a thread de conversão.
    - `OpenSource` guarda o stream, o nome do dispositivo e o id da saída usada; o drop para tudo.
  - `devices::should_reopen(current_output: &str, default_output_now: Option<&str>) -> bool`: função pura. Verdadeiro quando há um padrão novo diferente.
  - **Erros:**
    - dispositivo de microfone sumiu ou `DeviceNotAvailable` → `Failed("O microfone foi desconectado.")`;
    - erro no stream de sistema → a sessão tenta reabrir (Task 4);
    - permissão negada no Mac → `AppError::User` "Permita a gravação do som do computador em Ajustes do Sistema → Privacidade e Segurança → Gravação de áudio do sistema e tente de novo." (Microfone: o mesmo com "→ Microfone".)

- [ ] **Step 1: Testes que falham (puros).**
  - `resolve_mic`: wanted existente; wanted inexistente → padrão; sem padrão → primeiro; lista vazia → `None`.
  - `should_reopen`: mesmo id → false; id novo → true; `None` → false.
  - `list_inputs_does_not_panic` roda na máquina e no CI, só verifica `Ok` ou `Err` sem pânico. Os runners do CI não têm áudio, então não afirmar que a lista é não vazia.
- [ ] **Step 2:** `cargo test record::devices` → FAIL.
- [ ] **Step 3:** Implementar com a cpal 0.18.2 (ler `~/.cargo/registry/src/*/cpal-0.18.2/src/traits.rs` e `examples/`).
  - O som do computador é `host.default_output_device()` + `build_input_stream` nesse dispositivo de saída. No macOS isso cria o loopback; no Windows é o loopback WASAPI.
  - Aceitar os formatos f32, i16 e u16 convertendo para f32.
- [ ] **Step 4: Verificação manual-assistida na máquina de desenvolvimento.**
  - Um teste `#[ignore]` `real_capture_smoke` abre o microfone padrão e o sistema por 3 s e imprime o total de amostras e o nível.
  - Rode `cargo test real_capture_smoke -- --ignored --nocapture` enquanto toca um som. Registre a saída no relatório.
  - Se o macOS pedir permissão para o binário de teste, registre que pediu.
- [ ] **Step 5:** Commit `feat: dispositivos de áudio nativos (microfone e som do computador)`.

### Task 4: Sessão de gravação e comandos

**Files:**
- Create: `src-tauri/src/record/session.rs`, `src-tauri/src/commands/recording.rs`
- Modify: `src-tauri/src/commands/mod.rs`, `src-tauri/src/commands/transcription.rs` (remover `transcribe_chunk`; reaproveitar `validate_request`), `src-tauri/src/state.rs` (`recording: Mutex<Option<RecordingHandle>>`), `src-tauri/src/lib.rs` (registrar comandos, remover `transcribe_chunk`)

**Interfaces:**
- Consumes: Tasks 2–3.
- Produces:
  - Comandos:
    - `list_audio_inputs() -> Vec<AudioInput>`;
    - `start_recording(source: String, mic_id: Option<String>, model: String, language: String) -> Result<(), AppError>`;
    - `stop_recording() -> Result<Transcript, AppError>`.
  - Eventos:
    - `record://text { speaker: null | "voce" | "reuniao", text }`;
    - `record://level { mic: number | null, system: number | null }` a cada 100 ms;
    - `record://error { message }`.
  - `start_recording`:
    - valida o modelo e o idioma (`validate_request`), ocupa o slot `transcription_cancel` e salva `record_source`/`record_mic_id` em Settings;
    - abre as fontes e devolve logo após abrir; erro de abertura libera o slot.
  - `stop_recording`:
    - para as fontes, faz `finish` dos tracks e espera a fila esvaziar;
    - monta o `Transcript`: modo duplo → `interleave` + `labeled_text`; modo único → `join_text` e `speaker: None`;
    - libera o slot (também em erro ou pânico, com um guard RAII).
  - O `language` do resultado é o idioma pedido, como `transcribe_file` faz hoje.
- **Projeto interno** (o algoritmo que o teste não determina): a sessão é testável sem cpal.
  - `Session::new(sources: Vec<(Option<&'static str> /*speaker*/, Receiver<SourceMsg>)>, transcriber: Arc<dyn Fn(&[f32]) -> Result<Vec<Segment>, AppError> + Send + Sync>, sink: Arc<dyn Fn(SessionEvent) + Send + Sync>)`.
  - Uma thread de coleta, a cada 100 ms:
    - drena os canais para os `Track`s e chama `pad_to(now)`;
    - emite o nível;
    - manda os blocos prontos para uma única thread de transcrição por canal ilimitado (um Whisper por vez, a fila pode crescer);
    - em `Failed` de uma fonte de microfone, emite `Error` e encerra como em `stop`.
  - Para a fonte de sistema, a thread de coleta checa `should_reopen` a cada 1 s e reabre (via um fecho `reopen` injetado). Um erro do stream de sistema também dispara a reabertura, até 3 tentativas seguidas antes de virar `Error`.
- [ ] **Step 1: Testes que falham** com fontes fake (canais alimentados pelo teste) e um transcritor fake que devolve um segmento `"00:00:01,000"` com texto `b{índice}`:
  - `single_source_produces_plain_text`: 25 s → texto `"b0 b1 b2"`, `speaker` `None`.
  - `dual_source_interleaves_and_labels`: o texto tem linhas `Você:`/`Reunião:` em ordem de tempo.
  - `silent_system_source_keeps_clock`: sistema mudo por 24 s e depois 1 s de dados → o segmento de Reunião cai no bloco 2 (≥ 00:00:24).
  - `slow_transcriber_loses_nothing`: o transcritor dorme 50 ms por bloco e 10 blocos são enfileirados de uma vez → 10 resultados, nunca dois em paralelo (contador atômico de concorrência ≤ 1).
  - `mic_failure_stops_with_partial_result`: depois de 1 bloco, `Failed` → evento `Error` e um resultado com o texto do bloco 0.
  - `system_reopens_on_default_change`: o fecho `reopen` fake é chamado quando a saída padrão fake muda.
  - `recording_slot_blocks_file_transcription`: ocupa o slot e `transcribe_file` devolve o erro de exclusividade (testar via a função de claim do slot, extraída para ser testável).
- [ ] **Step 2:** `cargo test record::session commands::recording` → FAIL.
- [ ] **Step 3:** Implementar. O transcritor real é um fecho sobre `Engine::transcribe(model_path, model, pcm, language, cancel, noop)`. O cancel é o do slot; `stop` não cancela, só espera.
- [ ] **Step 4:**
  - `cargo test` → PASS, sem warnings.
  - `#[ignore]` `real_recording_smoke`: grava 15 s do microfone padrão com o modelo instalado (`LT_MODELS_DIR`, ou o diretório padrão do app) e imprime o `Transcript`. Rodar e registrar.
- [ ] **Step 5:** Commit `feat: sessão de gravação nativa com fila única e etiquetas`.

### Task 5: Interface: aba Gravar, indicador de gravação, exportação etiquetada

**Files:**
- Create: `ui/js/recorder.js` (funções puras + ligação com a API), `ui/tests/recorder.test.js`
- Modify:
  - `ui/js/api.js`: `listAudioInputs`, `startRecording`, `stopRecording`; remover `transcribeChunk`.
  - `ui/js/views/step-audio.js`: aba "Gravar" no lugar de "Microfone".
  - `ui/js/export.js`: o SRT prefixa `Você: `/`Reunião: ` quando `speaker` existe.
  - `ui/css/app.css`: indicador e barras.
  - `ui/tests/export.test.js`.
  - `README.md`: seção de gravação, permissão do som do computador, aviso de fone, Bluetooth.
- Delete: `ui/js/mic.js`

**Interfaces:**
- Consumes: os comandos e eventos da Task 4.
- Produces (em `recorder.js`, testáveis):
  - `SPEAKER_LABEL = { voce: 'Você', reuniao: 'Reunião' }`;
  - `liveLine({speaker, text}) -> string`: `"Você: texto"`, ou só o texto sem etiqueta;
  - `levelPercent(x) -> number`: 0–100 inteiro, com clamp;
  - `pickSource(settings) -> 'mic'|'system'|'both'`: `'mic'` quando ausente ou inválido;
  - `showMicPicker(source) -> boolean`;
  - `showHeadphoneHint(source) -> boolean`.

- [ ] **Step 1: Testes que falham** (`node --test`):
  - `liveLine` com e sem etiqueta;
  - `levelPercent(-1|0|0.5|2)` → `0|0|50|100`;
  - `pickSource(null|{record_source:'x'}|{record_source:'both'})`;
  - `showMicPicker('system') === false`;
  - `showHeadphoneHint('both') === true` e `false` para os outros;
  - `toSrt` com `speaker: 'reuniao'` produz a linha `Reunião: Oi.`, e sem `speaker` fica idêntico ao de hoje.
- [ ] **Step 2:** `npm run test:ui` → FAIL.
- [ ] **Step 3:** Implementar a tela conforme o esboço do design:
  - rádios de fonte;
  - `<select>` de microfone com o padrão marcado como "(padrão)", mais um botão ↻ que chama `listAudioInputs` (carrega ao abrir a aba);
  - aviso de fone com o texto exato;
  - cronômetro e indicador: `● Gravando` com a bolinha vermelha pulsando via CSS `@keyframes`, sem pulsar com `prefers-reduced-motion: reduce`;
  - uma barra de nível por fonte ativa, atualizada por `record://level`;
  - linhas ao vivo de `record://text`;
  - em "Finalizando…", a bolinha para e as barras somem;
  - `record://error` mostra a mensagem no banner. A gravação já terminou no backend, e o resultado parcial chega pelo `stopRecording` que a tela chama em seguida.
  - Mantidos: a confirmação "Substituir", a trava de tarefa (`startJob`/`failJob`/`afterTranscribe`) e o `JOB_LABEL.mic` "Gravando…".
  - Remover toda referência a `mic.js` e `getUserMedia`.
- [ ] **Step 4:**
  - `npm run test:ui` → PASS e `cargo test` → PASS;
  - `npm run tauri build` gera o `.dmg`;
  - abrir o app gerado por 5 s para confirmar que não trava;
  - push da branch `feat/captura-audio` e CI verde nos dois sistemas.
- [ ] **Step 5:** Commit `feat: aba Gravar com fonte, microfone, indicador e etiquetas`.

## Manual checklist (para o autor, ao final)

- **Mac:**
  - escolher um microfone que não seja o padrão;
  - Som do computador com um vídeo tocando;
  - os dois numa chamada real com fone (etiquetas em ordem);
  - pedido de permissão do som do computador, e o que acontece ao negar;
  - conectar o fone no meio (Reunião continua);
  - desconectar o microfone USB no meio (para com texto parcial);
  - indicador pulsando e barras se mexendo;
  - SRT e TXT etiquetados;
  - resumo do texto etiquetado.
- **Windows:** os mesmos casos (sem pedido de permissão), incluindo o loopback sem nada tocando por 1 minuto.
