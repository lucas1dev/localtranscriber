"""Multi-provider summary engine for Local Transcriber."""
from __future__ import annotations
import os, json, asyncio
from typing import Any
import httpx

PROVIDERS = {
    "openai": {"label": "ChatGPT / OpenAI", "default_model": "gpt-5.1", "auth": "api_key_or_sign_in_with_chatgpt"},
    "anthropic": {"label": "Claude", "default_model": "claude-sonnet-4-5", "auth": "api_key_or_linked_billing"},
    "gemini": {"label": "Gemini", "default_model": "gemini-2.5-flash", "auth": "api_key"},
    "ollama": {"label": "Ollama (local)", "default_model": "qwen3:8b", "auth": "local"},
}

STYLE_PROMPTS = {
    "executive": "Crie um resumo executivo conciso, com contexto, ideias centrais e conclusão.",
    "detailed": "Crie um resumo detalhado, organizado em seções e subtópicos, preservando nuances e argumentos.",
    "academic": "Produza uma síntese acadêmica: tema, objetivo, conceitos, metodologia (se mencionada), argumentos, evidências e conclusões.",
    "meeting": "Organize como ata: contexto, assuntos discutidos, decisões explícitas, tarefas, responsáveis e prazos. Não invente responsáveis ou prazos.",
    "class": "Organize como material de estudo: tema, objetivos, explicações, conceitos-chave, exemplos, dúvidas e revisão final.",
    "podcast": "Resuma como episódio de podcast: tema, ideias principais, momentos relevantes, opiniões e conclusões.",
    "custom": "Siga cuidadosamente as instruções personalizadas fornecidas pelo usuário.",
}

SYSTEM = """Você é um assistente especializado em análise de transcrições. Trabalhe somente com o conteúdo fornecido. Não invente fatos, nomes, decisões, citações, responsáveis ou timestamps. Diferencie afirmações explícitas de inferências. Preserve nomes próprios e termos técnicos. Quando houver timestamps, associe pontos importantes aos tempos correspondentes. Responda em português brasileiro, salvo pedido contrário. Retorne Markdown claro."""

async def _openai(text: str, model: str, prompt: str, api_key: str) -> str:
    payload = {"model": model, "instructions": SYSTEM, "input": prompt + "\n\nTRANSCRIÇÃO:\n" + text}
    async with httpx.AsyncClient(timeout=240) as client:
        r = await client.post("https://api.openai.com/v1/responses", headers={"Authorization": f"Bearer {api_key}"}, json=payload)
        r.raise_for_status(); data = r.json()
    out=[]
    for item in data.get("output", []):
        for c in item.get("content", []):
            if c.get("type") == "output_text": out.append(c.get("text", ""))
    return "\n".join(out).strip()

async def _anthropic(text: str, model: str, prompt: str, api_key: str) -> str:
    payload = {"model": model, "max_tokens": 6000, "system": SYSTEM, "messages": [{"role":"user", "content": prompt + "\n\nTRANSCRIÇÃO:\n" + text}]}
    async with httpx.AsyncClient(timeout=240) as client:
        r = await client.post("https://api.anthropic.com/v1/messages", headers={"x-api-key":api_key,"anthropic-version":"2023-06-01","content-type":"application/json"}, json=payload)
        r.raise_for_status(); data=r.json()
    return "\n".join(x.get("text","") for x in data.get("content",[]) if x.get("type")=="text").strip()

async def _gemini(text: str, model: str, prompt: str, api_key: str) -> str:
    url=f"https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
    payload={"system_instruction":{"parts":[{"text":SYSTEM}]},"contents":[{"parts":[{"text":prompt+"\n\nTRANSCRIÇÃO:\n"+text}]}],"generationConfig":{"temperature":0.2}}
    async with httpx.AsyncClient(timeout=240) as client:
        r=await client.post(url,headers={"x-goog-api-key":api_key},json=payload); r.raise_for_status(); data=r.json()
    return "\n".join(p.get("text","") for c in data.get("candidates",[]) for p in c.get("content",{}).get("parts",[])).strip()

async def _ollama(text: str, model: str, prompt: str, base_url: str) -> str:
    url=base_url.rstrip("/")+"/api/chat"
    payload={"model":model,"stream":False,"messages":[{"role":"system","content":SYSTEM},{"role":"user","content":prompt+"\n\nTRANSCRIÇÃO:\n"+text}],"options":{"temperature":0.2}}
    async with httpx.AsyncClient(timeout=600) as client:
        r=await client.post(url,json=payload); r.raise_for_status(); data=r.json()
    return data.get("message",{}).get("content","").strip()

async def _invoke(provider: str, text: str, model: str, prompt: str, api_key: str|None, ollama_url: str) -> str:
    if provider=="openai":
        if not api_key: raise ValueError("Configure uma chave da API OpenAI ou conecte Sign in with ChatGPT quando a integração OAuth estiver habilitada.")
        return await _openai(text,model,prompt,api_key)
    if provider=="anthropic":
        if not api_key: raise ValueError("Configure uma chave da Anthropic Console. Assinaturas Claude.ai não são credenciais de API.")
        return await _anthropic(text,model,prompt,api_key)
    if provider=="gemini":
        if not api_key: raise ValueError("Configure uma chave da Gemini API. A assinatura Google AI não substitui a credencial da API.")
        return await _gemini(text,model,prompt,api_key)
    return await _ollama(text,model,prompt,ollama_url)

def _split_text(text: str, limit: int=22000) -> list[str]:
    # Split on paragraph/sentence boundaries where possible; do not silently truncate.
    chunks=[]; current=""
    for paragraph in text.splitlines():
        paragraph=paragraph.strip()
        if not paragraph: continue
        while len(paragraph)>limit:
            cut=max(paragraph.rfind(". ",0,limit),paragraph.rfind("? ",0,limit),paragraph.rfind("! ",0,limit))
            if cut<limit//2: cut=limit
            chunks.append(paragraph[:cut+1].strip()); paragraph=paragraph[cut+1:].strip()
        if current and len(current)+len(paragraph)+1>limit:
            chunks.append(current); current=paragraph
        else: current=(current+"\n"+paragraph).strip()
    if current: chunks.append(current)
    return chunks or [text]

async def summarize(provider: str, text: str, style: str="executive", model: str|None=None, api_key: str|None=None, ollama_url: str="http://127.0.0.1:11434", custom_instructions: str="") -> dict[str, Any]:
    if provider not in PROVIDERS: raise ValueError("Provedor desconhecido")
    if not text.strip(): raise ValueError("A transcrição está vazia")
    if style not in STYLE_PROMPTS: raise ValueError("Formato de resumo inválido")
    model=model or PROVIDERS[provider]["default_model"]
    prompt=STYLE_PROMPTS[style]
    if style=="custom" and custom_instructions.strip(): prompt += "\nInstruções do usuário: " + custom_instructions.strip()
    chunks=_split_text(text)
    if len(chunks)==1:
        result=await _invoke(provider,chunks[0],model,prompt,api_key,ollama_url)
    else:
        # Map-reduce: summarize each chunk, then consolidate the intermediate notes.
        partials=[]
        for index,chunk in enumerate(chunks,1):
            partial_prompt=("Resuma este trecho mantendo fatos, nomes, conceitos e referências temporais. Não conclua o conteúdo global ainda. Identifique como trecho " + str(index) + " de " + str(len(chunks)) + ".\n" + prompt)
            partial=await _invoke(provider,chunk,model,partial_prompt,api_key,ollama_url)
            partials.append("TRECHO "+str(index)+"\n"+partial)
        result=await _invoke(provider,"\n\n".join(partials),model,"Consolide as sínteses parciais em um único resultado coerente, elimine repetições sem perder informações importantes e sinalize divergências. "+prompt,api_key,ollama_url)
    return {"provider":provider,"model":model,"style":style,"summary":result,"chunks_processed":len(chunks)}

async def compare(text: str, providers: list[str], style: str, models: dict[str,str], keys: dict[str,str], ollama_url: str, custom_instructions: str="") -> list[dict[str,Any]]:
    async def run(p):
        try: return await summarize(p,text,style,models.get(p),keys.get(p),ollama_url,custom_instructions)
        except Exception as e: return {"provider":p,"model":models.get(p) or PROVIDERS.get(p,{}).get("default_model"),"error":str(e)}
    return await asyncio.gather(*(run(p) for p in providers))
