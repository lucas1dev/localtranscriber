# Captura de áudio: escolher microfone e gravar o som do computador

Status: rascunho para aprovação do autor.

## Intenção

Hoje a aba "Microfone" do passo ① grava sempre o microfone padrão do sistema, dentro da janela do app (`getUserMedia` + `MediaRecorder`). O autor quer:

- escolher **qual microfone** usar;
- gravar o **som que sai do computador**, para transcrever uma reunião (Meet, Zoom, Teams) sem captar pelo ar;
- gravar **os dois juntos**, com a transcrição dizendo quem falou: "Você" (microfone) e "Reunião" (som do computador).

O motivo é transcrever e resumir reuniões. Saber o que foi dito por você e o que foi dito pelos outros melhora o resumo, por exemplo nas pendências ("fiquei de enviar X").

## Objetivos

1. Uma aba **Gravar** no passo ① substitui a aba "Microfone", com três fontes:
   - **Microfone**, com uma lista para escolher o dispositivo (o padrão do sistema vem primeiro);
   - **Som do computador**;
   - **Microfone + som do computador**.
2. No modo duplo, cada fonte é transcrita separadamente e os trechos são intercalados por tempo, com etiqueta:
   ```
   [00:12] Você: Bom dia, vamos começar pela pauta.
   [00:15] Reunião: Oi, tudo bem? Eu trouxe os números.
   ```
3. Texto ao vivo durante a gravação, como hoje, já com as etiquetas no modo duplo.
4. O app lembra a última fonte e o último microfone escolhidos.
5. Mac e Windows, tudo embutido, sem instalar driver ou programa extra.

## Fora do escopo

- Separar os participantes remotos entre si (diarização). "Reunião" é todo mundo do outro lado.
- Capturar o som de um app específico. É sempre o som do sistema inteiro.
- Salvar o arquivo de áudio gravado.
- Cancelamento de eco e remoção de falas duplicadas quando não há fone. O app só avisa.

## Anti-objetivos (seria um fracasso mesmo funcionando)

- Pedir para instalar BlackHole, Loopback ou outro dispositivo virtual.
- Um app que deixa de abrir em macOS anterior a 14.2. Hoje o mínimo é 11.0; nesses sistemas, só "Som do computador" e "Microfone + som do computador" ficam indisponíveis.
- "Você" e "Reunião" fora de ordem por usarem relógios diferentes.
- Perder o que já foi transcrito quando um dispositivo some no meio da gravação.

## Abordagens consideradas

1. **Tudo nativo em Rust** (escolhida pelo autor). O backend captura as duas fontes:
   - microfone e lista de microfones pela crate `cpal`;
   - som do computador no Mac por Core Audio *process tap* (macOS 14.2+);
   - som do computador no Windows por WASAPI loopback (`cpal` aceita abrir um stream de entrada sobre o dispositivo de saída).

   Um relógio só, PCM direto para o Whisper, sem FFmpeg por bloco. Custa reescrever a gravação do microfone, e resolve a dúvida que ficou aberta sobre o microfone dentro da janela do app no Mac.
2. **Misto.** O microfone continua na janela do app com `deviceId` e só o som do computador vai para o Rust. Mexe menos, mas são dois relógios e dois caminhos de gravação. Rejeitada pela ordem imprecisa entre as fontes.
3. **Dispositivo virtual** (BlackHole e afins). Descartada: o autor quer tudo embutido.

## Decisões do autor

- Os três modos: Microfone, Som do computador, Microfone + som do computador.
- No modo duplo, opção **B**: fontes transcritas separadamente, intercaladas por tempo, com as etiquetas "Você" e "Reunião".
- Captura nativa em Rust (abordagem 1).
- Aviso para usar fone de ouvido no modo duplo.

## Desenho

### Captura (Rust)

- Um módulo de captura abre as fontes e entrega PCM **mono f32 a 16 kHz** para o Whisper.
  - Converte canais e taxa: os dispositivos costumam trabalhar a 44,1 ou 48 kHz, em estéreo.
  - O callback de áudio não pode travar: só copia as amostras para um buffer; o resto acontece em outra thread.
- **Relógio comum.** As duas fontes começam juntas. O tempo de cada amostra é contado pelo número de amostras desde o início da sua fonte, ancorado no mesmo instante inicial.
  - Se uma fonte para de entregar áudio (no Windows, o loopback não manda nada quando não há som tocando), o trecho correspondente conta como silêncio. Os tempos dela não podem "encolher" e passar à frente da outra fonte.
  - *Decisão do designer:* cada fonte compara as amostras recebidas com o tempo real decorrido desde o instante inicial. Quando fica atrás por mais de 200 ms, completa a diferença com zeros. Pequenas variações de relógio abaixo disso são ignoradas.
- **Blocos.** Cada fonte é cortada em blocos de 12 s, como hoje, e cada bloco vai para `Engine::transcribe(pcm)`.
  - O deslocamento de tempo é o índice do bloco × 12 s, igual a `shiftTimestamp` hoje.
  - Os blocos entram numa fila e são transcritos um por vez, sem dois Whisper em paralelo. No modo duplo, os blocos das duas fontes se alternam na fila.
  - Se a fila crescer (modelo lento demais para tempo real), a gravação continua e o texto atrasa. Ao parar, a fila é esvaziada antes do resultado, como hoje.
  - Bloco silencioso: o filtro de energia já devolve zero segmentos, sem texto e sem custo de Whisper.
- **Mac, som do computador.** Process tap global do sistema (todos os processos, saída mixada para mono), dentro de um dispositivo agregado privado, lido por um callback de IO.
  - O app continua abrindo no macOS 11. As APIs de tap são resolvidas só em tempo de execução, quando o sistema é 14.2+. Como fazer isso (link fraco, `dlsym`, verificação de versão) fica com o construtor.
  - `Info.plist` ganha `NSAudioCaptureUsageDescription` em pt-BR.
  - Na primeira captura, o macOS pede permissão. Se for negada, a mensagem explica onde liberar.
- **Mac, microfone.** `cpal` sobre o dispositivo escolhido. A permissão de microfone já está declarada no `Info.plist`.
- **Windows.** O microfone usa `cpal` (WASAPI), e o som do computador usa loopback do dispositivo de saída padrão via `cpal`. Não há pedido de permissão.
- **Dispositivo some** (fone desconectado, por exemplo): a gravação para com uma mensagem clara, e tudo o que já foi transcrito vai para o passo ② normalmente.
- **Saída de som trocada no meio da gravação** (por exemplo, a pessoa conecta o fone depois que a reunião começou). *Decisão do designer:* o som do computador acompanha a nova saída, sem parar a gravação.
  - No Mac, o tap global já captura o que os processos tocam, seja qual for a saída. Se o dispositivo agregado precisar ser refeito, refaz.
  - No Windows, ao mudar a saída padrão, o loopback é reaberto no novo dispositivo.
  - Um intervalo curto de silêncio na troca é aceitável. O relógio comum preenche esse intervalo.
  - Só o microfone escolhido sumir para a gravação.
- **Fone Bluetooth** (por exemplo AirPods). Quando o microfone dele é ligado, o sistema troca a saída para o modo de chamada, que tem qualidade menor. A gravação continua funcionando. Basta o README mencionar isso. Não precisa de tratamento especial.

### Comandos e eventos (contrato com a interface)

- `list_audio_inputs() -> [{ id, name, is_default }]`
  - `id` é um identificador estável entre execuções (o nome ou o UID do dispositivo).
- `capture_capabilities() -> { system_audio: bool, reason: Option<String> }`
  - `system_audio` é falso no macOS anterior a 14.2, com o motivo em pt-BR.
- `start_recording({ source: "mic" | "system" | "both", mic_id: Option<String>, model, language })`
  - Ocupa o mesmo espaço de tarefa da transcrição de arquivo: bloqueia apagar e importar modelos, como hoje.
- `stop_recording() -> Transcript`
  - Resolve depois de transcrever o que ainda está na fila.
- Evento `record://text { speaker: Option<"voce" | "reuniao">, text }` a cada bloco transcrito.
- Evento `record://error { message }` quando uma fonte falha no meio da gravação. Depois disso a gravação para.
- `Segment` ganha `speaker: Option<String>`. Fica `None` em arquivo, só microfone e só som do computador; recebe o valor no modo duplo.
- Saem `transcribe_chunk` e `ui/js/mic.js` (o caminho via `MediaRecorder`).
- `sourceName` do resultado:
  - "Microfone — <nome do dispositivo>";
  - "Som do computador";
  - "Microfone + som do computador".

### Texto, exportação e resumo

- No modo duplo, `Transcript.text` é montado linha a linha, em ordem de tempo, no formato `[mm:ss] Você: texto` ou `[mm:ss] Reunião: texto`.
  - Acima de uma hora, o tempo vira `[h:mm:ss]`.
  - Segmentos seguidos da mesma fonte se juntam na mesma linha, com o tempo do primeiro.
  - O autor aprovou a opção B vendo esse formato.
  - Os outros modos (arquivo, só microfone, só som do computador) continuam como hoje: texto corrido, sem tempo.
- O texto ao vivo na tela mostra só `Você:` ou `Reunião:`, sem o tempo.
- O SRT põe o prefixo no texto de cada legenda. TXT e Markdown usam o texto já etiquetado.
- O resumo recebe o texto etiquetado. Os prompts copiados do Python não mudam; as etiquetas no texto bastam.

### Interface (passo ①)

```
[ Arquivo ] [ Gravar ]

Fonte:   (•) Microfone   ( ) Som do computador   ( ) Microfone + som do computador
Microfone: [ MacBook Pro Microfone (padrão)  ▾ ]      ← some no modo "Som do computador"
ⓘ Use fone de ouvido para a fala dos outros não ser gravada duas vezes.   ← só no modo duplo

00:42
Você: Bom dia, vamos começar pela pauta.
Reunião: Oi, tudo bem? Eu trouxe os números.

[ Parar ]
```

- Sem suporte a som do computador, as duas opções aparecem desativadas, com o motivo em texto pequeno.
- A lista de microfones é carregada ao abrir a aba. Um botão ↻ recarrega a lista.
- A fonte e o microfone escolhidos ficam salvos em `Settings`:
  - campos novos `record_source` e `record_mic_id`, com leitura tolerante igual à do `summary_provider`;
  - se o microfone salvo não existir mais, usa o padrão do sistema.
- O resto da tela fica como está: modelo, idioma, cronômetro, "Substituir" e a trava de tarefa.

## Testes

- **Automáticos:**
  - conversão de canais e taxa para 16 kHz;
  - corte em blocos de 12 s e deslocamento de tempo;
  - relógio comum quando uma fonte fica sem áudio;
  - intercalação e etiquetas: ordem por tempo, empate, segmentos seguidos da mesma fonte;
  - texto, SRT e TXT com etiquetas;
  - leitura tolerante dos campos novos de `Settings`;
  - `capture_capabilities` por versão do macOS, numa função pura que recebe a versão;
  - a fila não roda dois Whisper ao mesmo tempo.
- **Manuais:**
  - Mac: escolher um microfone que não seja o padrão;
  - Mac: som do computador com um vídeo do YouTube tocando;
  - Mac: os dois juntos numa chamada real, com fone;
  - Mac: pedido de permissão e negação;
  - Windows: os mesmos casos;
  - desconectar o fone no meio da gravação;
  - macOS anterior a 14.2: opções desativadas, se houver uma máquina disponível.

## O que fica com o construtor

- A crate e a forma de chamar o Core Audio tap. Exemplos: `objc2-core-audio`, ou o suporte de loopback que versões recentes do `cpal` podem ter no macOS; verificar antes de escrever bindings próprios.
- Como o app abre no macOS 11 sem as APIs de tap.
- A biblioteca ou o algoritmo de reamostragem.
- O formato exato de `id` do microfone.
- A estrutura interna dos módulos, seguindo a organização atual (`src-tauri/src/`, um assunto por arquivo).
