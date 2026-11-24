# (Arquivo so em ASCII: o Windows PowerShell 5.1 le .ps1 sem BOM como ANSI.)
# Confere os sidecars do Windows em src-tauri\binaries:
#   - ffmpeg e codex presentes;
#   - ffmpeg na versao do lock e sem --enable-gpl ou --enable-nonfree na linha
#     "configuration:" do `ffmpeg -version`. O --enable-version3 e esperado: o
#     build win64-lgpl do BtbN e LGPL v3 (veja ffmpeg.<triple>.license no lock);
#   - codex --version igual a versao do lock.
# Rode depois de scripts\fetch-sidecars.ps1. Sai com codigo != 0 se algo falhar.
# No macOS, use scripts/verify-sidecars.sh.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Triple = 'x86_64-pc-windows-msvc'
$Lock = Get-Content -Raw -LiteralPath (Join-Path $Root 'scripts\sidecars.lock.json') | ConvertFrom-Json
$Bin = Join-Path $Root 'src-tauri\binaries'
$Ffmpeg = Join-Path $Bin "ffmpeg-$Triple.exe"
$Codex = Join-Path $Bin "codex-$Triple.exe"

$script:failures = 0
function Fail([string]$Message) {
    [Console]::Error.WriteLine("FALHA: $Message")
    $script:failures++
}
function Ok([string]$Message) { Write-Host "ok: $Message" }
function Exit-IfFailed {
    if ($script:failures -ne 0) {
        [Console]::Error.WriteLine("$($script:failures) verificacao(oes) falharam.")
        exit 1
    }
}

foreach ($exe in @($Ffmpeg, $Codex)) {
    if (Test-Path -LiteralPath $exe -PathType Leaf) { Ok "presente: $(Split-Path -Leaf $exe)" }
    else { Fail "sidecar ausente: $exe" }
}
Exit-IfFailed

# --- FFmpeg: versao e licenca -------------------------------------------------
$ffmpegOut = @(& $Ffmpeg -hide_banner -version)
if ($LASTEXITCODE -ne 0) { Fail "ffmpeg -version saiu com codigo $LASTEXITCODE" }
$ffmpegLockVersion = $Lock.ffmpeg.$Triple.version
$firstLine = if ($ffmpegOut.Count -gt 0) { $ffmpegOut[0] } else { '' }
if ($firstLine.StartsWith("ffmpeg version $ffmpegLockVersion-") -or $firstLine.StartsWith("ffmpeg version $ffmpegLockVersion ")) {
    Ok "ffmpeg $ffmpegLockVersion"
}
else {
    Fail "versao do ffmpeg: esperado $ffmpegLockVersion, obtido: $firstLine"
}
$configuration = @($ffmpegOut | Where-Object { $_ -like 'configuration:*' })
if ($configuration.Count -ne 1) {
    Fail "ffmpeg -version sem a linha 'configuration:'"
}
else {
    $forbidden = @('--enable-gpl', '--enable-nonfree') |
        Where-Object { $configuration[0].Contains($_) }
    if ($forbidden) { Fail "ffmpeg foi configurado com $($forbidden -join ', ')" }
    else { Ok 'ffmpeg sem --enable-gpl ou --enable-nonfree' }
}

# --- Codex: versao ------------------------------------------------------------
$codexLockVersion = $Lock.codex.version
$codexHome = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force -Path $codexHome | Out-Null
$previousCodexHome = $env:CODEX_HOME
try {
    $env:CODEX_HOME = $codexHome
    $codexOut = (@(& $Codex --version) -join "`n").Trim()
}
finally {
    $env:CODEX_HOME = $previousCodexHome
    Remove-Item -Recurse -Force -LiteralPath $codexHome -ErrorAction SilentlyContinue
}
if ($codexOut -eq "codex-cli $codexLockVersion") { Ok $codexOut }
else { Fail "versao do codex: esperado codex-cli $codexLockVersion, obtido: $codexOut" }

Exit-IfFailed
Write-Host 'Sidecars OK.'
