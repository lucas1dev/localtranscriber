# Local Transcriber 2.0

Aplicação local para transcrição com Whisper.cpp e resumo multi-provedor (OpenAI, Anthropic, Gemini e Ollama).

## Arquitetura

- **Frontend:** HTML/CSS/JavaScript servido pelo FastAPI.
- **Backend:** FastAPI em `127.0.0.1:8765`.
- **Transcrição:** `whisper-cli` (whisper.cpp), com Metal quando a build do Homebrew estiver habilitada para Apple Silicon.
- **Modelo principal:** Whisper Large V3 Turbo (`ggml-large-v3-turbo.bin`).
- **Conversão:** FFmpeg para WAV mono 16 kHz.
- **Resumos:** adaptadores HTTP separados para OpenAI Responses API, Anthropic Messages API, Gemini generateContent e Ollama `/api/chat`.
- **Credenciais:** macOS Keychain via Python Keyring; chaves nunca são devolvidas à interface.
- **Histórico:** nesta entrega os resultados ficam na sessão do navegador; persistência SQLite e biblioteca de projetos ficam para a próxima etapa.
- **Privacidade:** áudio/transcrição só saem do Mac quando o usuário explicitamente aciona um provedor de nuvem para resumir. Ollama permanece local.

## Requisitos

- macOS Apple Silicon (testado em projeto para M4; valide a build Metal no seu computador).
- Homebrew.
- Python 3.12.
- Espaço em disco para modelo (Large V3 Turbo ocupa vários GB, dependendo da distribuição).
- Ollama opcional para resumo local.

## Instalação

```bash
chmod +x install.sh start.sh
./install.sh large-v3-turbo
```

O instalador instala `whisper.cpp`, FFmpeg, Python 3.12, dependências Python e baixa o modelo GGML Large V3 Turbo.

Modelos alternativos:

```bash
./install.sh small
./install.sh medium
./install.sh base
```

## Executar

```bash
./start.sh
```

Abra `http://127.0.0.1:8765` no Safari ou Chrome. Pare com `Ctrl+C` no Terminal.

## Desenvolvimento (app desktop, `src-tauri/`)

Antes de `cargo build`, `cargo test` ou `npm run tauri build`, prepare os sidecars (FFmpeg LGPL e Codex): `scripts/fetch-sidecars.sh` no macOS ou `scripts/fetch-sidecars.ps1` no Windows. Sem eles o build falha com ``resource path `binaries/codex-...` doesn't exist``. Detalhes, versões e licenças em [`scripts/README-sidecars.md`](scripts/README-sidecars.md).

## Transcrição

- Upload de áudio/vídeo e conversão via FFmpeg.
- Gravação via MediaRecorder em blocos curtos (quase em tempo real; não é streaming palavra a palavra).
- O modelo padrão da interface é Large V3 Turbo.
- A transcrição é executada localmente.

## AI Summary Engine

1. Selecione o provedor e formato.
2. Para OpenAI, Anthropic ou Gemini, salve uma chave de API. Ela é guardada no macOS Keychain.
3. Para Ollama, instale e inicie o Ollama e baixe um modelo, por exemplo:

```bash
ollama pull qwen3:8b
```

4. Gere um resumo ou compare os quatro provedores em paralelo.

Formatos incluídos: executivo, detalhado, acadêmico, reunião/ata, aula/estudo, podcast e personalizado.

Transcrições longas são divididas em blocos de até aproximadamente 22 mil caracteres, resumidas individualmente e consolidadas (map-reduce). Isso aumenta o número de chamadas e o custo nos provedores de nuvem.

### Autenticação: o que a assinatura inclui

- **ChatGPT:** a integração por API key está implementada. A OpenAI também documenta Sign in with ChatGPT para aplicativos open-source locais elegíveis, com consentimento específico para uso do plano. Esse OAuth ainda requer registrar/configurar o cliente e cumprir o fluxo oficial; não está ativado nesta versão. Não reutilize cookies, senhas ou tokens da sessão web.
- **Claude:** use uma chave da Anthropic Console. Assinaturas Claude.ai Pro/Max não são chaves de API e não podem ser usadas como credencial de um aplicativo terceiro. Linked billing exige organização Claude Platform elegível, não assinatura Claude.ai.
- **Gemini:** use uma chave Gemini API. A assinatura Google AI e a cobrança/limites da Gemini API são produtos separados.
- **Ollama:** chamadas locais sem chave e sem envio do texto para a nuvem.

As chaves de API são cobradas conforme a conta/plano do respectivo provedor e podem gerar custos. Verifique limites e preços antes de comparar vários provedores.

## Endpoints

- `GET /api/status`
- `POST /api/transcribe` multipart: `file`, `model`, `language`
- `GET /api/providers`
- `PUT /api/providers/{openai|anthropic|gemini}/credential`
- `DELETE /api/providers/{openai|anthropic|gemini}/credential`
- `GET /api/providers/ollama/models`
- `POST /api/summarize`
- `POST /api/compare`

## Limitações conhecidas desta entrega

- OAuth Sign in with ChatGPT ainda não está ligado; API key OpenAI funciona.
- Claude Linked Billing/OAuth não está implementado; API key Anthropic funciona.
- Gemini usa API key.
- Comparação dispara chamadas aos quatro provedores selecionados, incluindo os que não estiverem configurados; os erros aparecem individualmente e não interrompem os outros.
- Gravação em blocos pode cortar frases nos limites. Para gravações longas com máxima continuidade, grave o arquivo completo e transcreva depois.
- O resumo envia o texto ao provedor selecionado. Não envie transcrições confidenciais para nuvem sem avaliar consentimento e privacidade.
- Não exponha o servidor na rede. Ele foi projetado para uso local e não tem autenticação multiusuário/CSRF para acesso remoto.

## Próximas evoluções

- OAuth Sign in with ChatGPT oficial para cliente open-source local.
- SQLite para histórico, projetos, transcrições e resumos.
- Gerenciador visual de modelos Whisper e Ollama.
- Exportação Markdown/JSON e resumo com timestamps/citações por segmento.
- Jobs em background, cancelamento e progresso de transcrições/resumos longos.
- Empacotamento como aplicativo macOS.
