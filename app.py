from fastapi import FastAPI, UploadFile, File, Form, HTTPException
from fastapi.responses import FileResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, Field
from pathlib import Path
import subprocess, tempfile, json, uuid, shutil
from typing import Optional
import keyring
from ai_engine import PROVIDERS, summarize, compare

ROOT=Path(__file__).parent; MODELS=ROOT/"models"; OUTPUTS=ROOT/"outputs"; OUTPUTS.mkdir(exist_ok=True)
SERVICE="local-transcriber-ai"
MODEL_MAP={"base":MODELS/"ggml-base.bin","small":MODELS/"ggml-small.bin","medium":MODELS/"ggml-medium.bin","large-v3-turbo":MODELS/"ggml-large-v3-turbo.bin"}
app=FastAPI(title="Local Transcriber",version="2.0.0")
app.mount("/static",StaticFiles(directory=ROOT/"static"),name="static")

@app.get("/")
def home(): return FileResponse(ROOT/"static"/"index.html")

@app.get("/api/status")
def status():
    cli=shutil.which("whisper-cli"); ffmpeg=shutil.which("ffmpeg")
    return {"ready":bool(cli and ffmpeg),"whisper":cli,"ffmpeg":ffmpeg,"models":{k:v.exists() for k,v in MODEL_MAP.items()},"version":"2.0.0"}

@app.post("/api/transcribe")
async def transcribe(file:UploadFile=File(...),model:str=Form("large-v3-turbo"),language:str=Form("auto")):
    if model not in MODEL_MAP: raise HTTPException(400,"Modelo inválido")
    model_path=MODEL_MAP[model]
    if not model_path.exists(): raise HTTPException(400,f"Modelo {model} não instalado. Execute ./install.sh {model}")
    suffix=Path(file.filename or "audio").suffix or ".audio"
    with tempfile.TemporaryDirectory(prefix="transcriber-") as td:
        source=Path(td)/f"input{suffix}"; wav=Path(td)/"audio.wav"; source.write_bytes(await file.read())
        conv=subprocess.run(["ffmpeg","-y","-i",str(source),"-ar","16000","-ac","1","-c:a","pcm_s16le",str(wav)],capture_output=True,text=True)
        if conv.returncode: raise HTTPException(400,"Não foi possível converter o áudio: "+conv.stderr[-500:])
        prefix=Path(td)/"result"; cmd=["whisper-cli","-m",str(model_path),"-f",str(wav),"-oj","-of",str(prefix),"-t","6"]
        if language!="auto": cmd += ["-l",language]
        run=subprocess.run(cmd,capture_output=True,text=True); jp=Path(str(prefix)+".json")
        if run.returncode or not jp.exists(): raise HTTPException(500,"Falha na transcrição: "+(run.stderr[-1200:] or run.stdout[-1200:]))
        raw=json.loads(jp.read_text()); segments=[]
        for item in raw.get("transcription",[]):
            ts=item.get("timestamps",{}); segments.append({"start":ts.get("from","00:00:00,000"),"end":ts.get("to","00:00:00,000"),"text":item.get("text","").strip()})
        return {"id":uuid.uuid4().hex,"filename":file.filename,"model":model,"language":language,"text":" ".join(s["text"] for s in segments).strip(),"segments":segments}

class CredentialIn(BaseModel): key:str=Field(min_length=8,max_length=500)
class SummaryIn(BaseModel):
    text:str=Field(min_length=1,max_length=500000)
    provider:str
    style:str="executive"
    model:Optional[str]=None
    custom_instructions:str=""
class CompareIn(BaseModel):
    text:str=Field(min_length=1,max_length=500000)
    providers:list[str]=Field(min_length=2,max_length=4)
    style:str="executive"
    models:dict[str,str]={}
    custom_instructions:str=""

@app.get("/api/providers")
def providers_status():
    result=[]
    for p,meta in PROVIDERS.items():
        try: has_key=bool(keyring.get_password(SERVICE,p))
        except Exception: has_key=False
        result.append({"id":p,"label":meta["label"],"default_model":meta["default_model"],"configured":has_key if p!="ollama" else True,"auth":meta["auth"]})
    return {"providers":result,"chatgpt_oauth":"not_configured","note":"Sign in with ChatGPT requires a registered open-source OAuth client and its official request contract; API-key mode is available separately."}

@app.put("/api/providers/{provider}/credential")
def save_credential(provider:str,body:CredentialIn):
    if provider not in ("openai","anthropic","gemini"): raise HTTPException(400,"Este provedor não utiliza API key")
    try: keyring.set_password(SERVICE,provider,body.key)
    except Exception as e: raise HTTPException(500,"Não foi possível salvar no macOS Keychain: "+str(e))
    return {"saved":True,"provider":provider}

@app.delete("/api/providers/{provider}/credential")
def delete_credential(provider:str):
    try: keyring.delete_password(SERVICE,provider)
    except keyring.errors.PasswordDeleteError: pass
    except Exception as e: raise HTTPException(500,str(e))
    return {"deleted":True,"provider":provider}

@app.get("/api/providers/ollama/models")
async def ollama_models():
    import httpx
    try:
        async with httpx.AsyncClient(timeout=4) as client:
            r=await client.get("http://127.0.0.1:11434/api/tags"); r.raise_for_status(); data=r.json()
        return {"available":True,"models":[m["name"] for m in data.get("models",[])]}
    except Exception: return {"available":False,"models":[]}

@app.post("/api/summarize")
async def make_summary(body:SummaryIn):
    if body.provider not in PROVIDERS: raise HTTPException(400,"Provedor inválido")
    key=None
    if body.provider!="ollama":
        try: key=keyring.get_password(SERVICE,body.provider)
        except Exception: pass
    try: return await summarize(body.provider,body.text,body.style,body.model,key,custom_instructions=body.custom_instructions)
    except ValueError as e: raise HTTPException(400,str(e))
    except Exception as e: raise HTTPException(502,f"Falha no provedor {body.provider}: {str(e)[:700]}")

@app.post("/api/compare")
async def compare_summaries(body:CompareIn):
    if any(p not in PROVIDERS for p in body.providers): raise HTTPException(400,"Provedor inválido")
    keys={}
    for p in body.providers:
        if p!="ollama":
            try: keys[p]=keyring.get_password(SERVICE,p) or ""
            except Exception: keys[p]=""
    results=await compare(body.text,body.providers,body.style,body.models,keys,"http://127.0.0.1:11434",body.custom_instructions)
    return {"results":results}
