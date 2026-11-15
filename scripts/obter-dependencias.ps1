# Baixa pra `runtime\` (onde a IDE do Aylis põe o reflex.exe) as bibliotecas
# que o Reflex carrega em tempo de execução e que não ficam no repositório:
#
#   - o FFmpeg 7.1 LGPL "shared" (BtbN/FFmpeg-Builds): os encoders de placa de
#     vídeo e todos os decoders. Sem x264 (GPL) — o H.264 pela CPU é o openh264;
#   - o openh264 da Cisco, pelo próprio reflex.exe (`--baixar-openh264`, que
#     confere o hash). Só pra desenvolvimento: numa instalação, o `--install`
#     baixa (e a publicação não o leva).
#
#   .\scripts\obter-dependencias.ps1
param([switch]$SemOpenH264)

$ErrorActionPreference = "Stop"
$raiz = Split-Path $PSScriptRoot -Parent
$runtime = Join-Path $raiz "runtime"
New-Item -ItemType Directory -Force $runtime | Out-Null

# FFmpeg 7.1 (avcodec-61, avutil-59: as versões que rust\src\media\ffi espera).
# Build datada (a "latest" do BtbN só tem as séries novas), com o hash fixo.
$url = "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-07-31-14-10/ffmpeg-n7.1.5-12-g1fdbca85aa-win64-lgpl-shared-7.1.zip"
$sha256 = "0F376F96FB38554CCEFB1B2AE9C7C6A7B351F0E60A372B38262C320E8392C5D0"
$dlls = @("avcodec-61.dll", "avutil-59.dll", "swresample-5.dll", "swscale-8.dll")
$tmp = Join-Path ([IO.Path]::GetTempPath()) "reflex-ffmpeg-$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    Write-Host "Baixando $url"
    $zip = Join-Path $tmp "ffmpeg.zip"
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash
    if ($sha256 -and $hash -ne $sha256) { throw "SHA256 do pacote não confere: $hash" }
    Write-Host "SHA256 do pacote: $hash"
    Expand-Archive $zip -DestinationPath $tmp
    $bin = Get-ChildItem $tmp -Recurse -Directory -Filter bin | Select-Object -First 1
    if (-not $bin) { throw "o pacote não tem a pasta bin" }
    foreach ($d in $dlls) {
        $src = Join-Path $bin.FullName $d
        if (-not (Test-Path $src)) { throw "o pacote não tem $d (versão do FFmpeg mudou?)" }
        Copy-Item $src $runtime -Force
    }
    Write-Host "FFmpeg LGPL em $runtime" -ForegroundColor Green
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not $SemOpenH264) {
    # O próprio Reflex baixa da Cisco e confere o hash (o PowerShell não abre .bz2).
    $ErrorActionPreference = "Continue"
    cargo run --release -q --manifest-path (Join-Path $raiz "rust\Cargo.toml") -- --baixar-openh264 $runtime
    $ErrorActionPreference = "Stop"
    if ($LASTEXITCODE -ne 0) { throw "não deu pra baixar o openh264" }
}
