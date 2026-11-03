# Compila o Reflex e monta dist\ com só o que a máquina precisa: o
# reflex.exe e as DLLs (FFmpeg, x264, runtime do C). É só copiar a pasta pra
# outra máquina e rodar `reflex.exe --install` como administrador.
#
#   .\scripts\empacotar.ps1              compila e monta
#   .\scripts\empacotar.ps1 -SemCompilar só monta (usa o target\release atual)
#
# O exe é COPIADO (5 MB): o Reflex aberto a partir de dist\ não trava o
# próximo build. As DLLs (42 MB) entram como hard link — o mesmo arquivo de
# third_party\ffmpeg, sem ocupar espaço de novo; copiar a pasta leva o conteúdo.
param([switch]$SemCompilar)

$ErrorActionPreference = "Stop"
$raiz = Split-Path $PSScriptRoot -Parent
$dlls = Join-Path $raiz "third_party\ffmpeg"
$exe = Join-Path $raiz "target\release\reflex.exe"
$destino = Join-Path $raiz "dist"

if (-not $SemCompilar) {
    # O cargo escreve o progresso no stderr; com "Stop" o PowerShell 5.1 acha
    # que é erro. Vale o código de saída.
    $ErrorActionPreference = "Continue"
    cargo build --release -p reflex --manifest-path (Join-Path $raiz "Cargo.toml")
    $ErrorActionPreference = "Stop"
    if ($LASTEXITCODE -ne 0) { throw "a compilação falhou" }
}
if (-not (Test-Path $exe)) { throw "não achei $exe; rode sem -SemCompilar" }

New-Item -ItemType Directory -Force $destino | Out-Null
$esperados = @("reflex.exe") + @(Get-ChildItem "$dlls\*.dll" | ForEach-Object Name)

# Tira o que sobrou de pacotes antigos.
Get-ChildItem $destino | Where-Object { $esperados -notcontains $_.Name } | Remove-Item -Recurse -Force

try {
    Copy-Item $exe $destino -Force
} catch {
    throw "não deu pra atualizar $destino\reflex.exe — feche o Reflex aberto dali e rode de novo"
}

foreach ($dll in Get-ChildItem "$dlls\*.dll") {
    $alvo = Join-Path $destino $dll.Name
    if (Test-Path $alvo) {
        $atual = Get-Item $alvo
        if ($atual.Length -eq $dll.Length -and $atual.LastWriteTimeUtc -eq $dll.LastWriteTimeUtc) { continue }
        Remove-Item $alvo -Force
    }
    try {
        New-Item -ItemType HardLink -Path $alvo -Target $dll.FullName | Out-Null
    } catch {
        Copy-Item $dll.FullName $alvo
    }
}

$total = (Get-ChildItem $destino -File | Measure-Object Length -Sum).Sum
Write-Host ("Pacote pronto: {0} ({1} arquivos, {2:N1} MB)" -f $destino, (Get-ChildItem $destino -File).Count, ($total / 1MB)) -ForegroundColor Green
