# (Arquivo so em ASCII: o Windows PowerShell 5.1 le .ps1 sem BOM como ANSI.)
# Prepara os sidecars do Windows (x86_64-pc-windows-msvc) em src-tauri\binaries:
#   - codex-x86_64-pc-windows-msvc.exe: da GitHub Release fixada no lock;
#   - ffmpeg-x86_64-pc-windows-msvc.exe: so o ffmpeg.exe do build LGPL do BtbN.
# Aborta com codigo != 0 se algum SHA-256 nao bater com o lock.
# No macOS, use scripts/fetch-sidecars.sh.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
# O Windows PowerShell 5.1 pode nao habilitar TLS 1.2 por padrao (GitHub exige).
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Root = Split-Path -Parent $PSScriptRoot
$Triple = 'x86_64-pc-windows-msvc'
$Lock = Get-Content -Raw -LiteralPath (Join-Path $Root 'scripts\sidecars.lock.json') | ConvertFrom-Json
$Bin = Join-Path $Root 'src-tauri\binaries'
$Cache = Join-Path $Root 'src-tauri\target\sidecars'

New-Item -ItemType Directory -Force -Path $Bin, $Cache | Out-Null

# Baixa $Url para $File (reaproveitando o cache) e confere o SHA-256.
function Get-VerifiedFile([string]$Url, [string]$File, [string]$Sha256) {
    if (-not (Test-Path -LiteralPath $File)) {
        Write-Host "Baixando $Url..."
        $part = "$File.part"
        Invoke-WebRequest -Uri $Url -OutFile $part -UseBasicParsing
        Move-Item -Force -LiteralPath $part -Destination $File
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $File).Hash.ToLowerInvariant()
    if ($actual -ne $Sha256.ToLowerInvariant()) {
        Remove-Item -Force -LiteralPath $File
        Write-Error "SHA-256 invalido para ${File}: esperado $Sha256, obtido $actual" -ErrorAction Continue
        exit 1
    }
}

# Extrai o zip $Archive e copia so o arquivo $Member para $Dest.
function Copy-FromZip([string]$Archive, [string]$Member, [string]$Dest) {
    $work = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    try {
        Expand-Archive -LiteralPath $Archive -DestinationPath $work -Force
        $source = Join-Path $work $Member
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            Write-Error "$Member nao encontrado em $Archive" -ErrorAction Continue
            exit 1
        }
        Copy-Item -Force -LiteralPath $source -Destination $Dest
    }
    finally {
        Remove-Item -Recurse -Force -LiteralPath $work -ErrorAction SilentlyContinue
    }
}

# --- Codex --------------------------------------------------------------------
$codex = $Lock.codex.$Triple
$codexArchive = Join-Path $Cache ("codex-{0}-{1}" -f $Lock.codex.version, (Split-Path -Leaf $codex.url))
Get-VerifiedFile $codex.url $codexArchive $codex.sha256
$codexDest = Join-Path $Bin "codex-$Triple.exe"
Copy-FromZip $codexArchive $codex.member $codexDest
Write-Host "Codex $($Lock.codex.version) -> $codexDest"

# --- FFmpeg -------------------------------------------------------------------
$ffmpeg = $Lock.ffmpeg.$Triple
$ffmpegArchive = Join-Path $Cache (Split-Path -Leaf $ffmpeg.url)
Get-VerifiedFile $ffmpeg.url $ffmpegArchive $ffmpeg.sha256
# O zip do BtbN tem uma pasta raiz com o nome do build (ex.: ffmpeg-n9.0.2-...\bin\ffmpeg.exe).
$ffmpegMember = Join-Path ([System.IO.Path]::GetFileNameWithoutExtension($ffmpegArchive)) $ffmpeg.member
$ffmpegDest = Join-Path $Bin "ffmpeg-$Triple.exe"
Copy-FromZip $ffmpegArchive $ffmpegMember $ffmpegDest
Write-Host "FFmpeg $($ffmpeg.version) -> $ffmpegDest"
