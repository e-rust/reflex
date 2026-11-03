#Requires -RunAsAdministrator
<#
.SYNOPSIS
  Instala o Reflex numa VM do Hyper-V e cadastra este PC como master.

.DESCRIPTION
  Roda no HOST (este PC), num PowerShell como administrador. Usa o PowerShell
  Direct (não precisa de rede nem de WinRM na VM):
    1. para o Reflex na VM, se já estiver lá;
    2. copia reflex.exe + DLLs do FFmpeg pra C:\Reflex na VM;
    3. `reflex --install` na VM (serviço, firewall, política do Ctrl+Alt+Del);
    4. cadastra o ID do viewer deste PC como master na VM;
    5. mostra o estado e o IP pra conectar.

  Com -Logs, só mostra o final dos logs da VM (pra diagnosticar).

.EXAMPLE
  .\scripts\testar-vm.ps1
  .\scripts\testar-vm.ps1 -VMName "Win11-Teste"
  .\scripts\testar-vm.ps1 -Logs
#>
param(
    # Nome da VM; vazio = a única VM ligada.
    [string]$VMName,
    [string]$User = "claude",
    [string]$Pasta = "C:\Reflex",
    [string]$Nome = $env:USERNAME,
    # ID do viewer a cadastrar como master; vazio = o deste PC (reflex --my-id).
    # Dentro do terminal do app Claude (MSIX) o AppData é virtualizado e o
    # --my-id sai diferente do Reflex aberto pelo Explorer: passe o ID mostrado
    # na aba Conectar.
    [string]$ViewerId,
    # Pasta com reflex.exe + DLLs (padrão: dist, montada por empacotar.ps1).
    [string]$Origem,
    [switch]$Logs
)

$ErrorActionPreference = "Stop"
if (-not $Origem) { $Origem = Join-Path $PSScriptRoot "..\dist" }
if (-not (Test-Path $Origem)) { throw "Monte o pacote antes: .\scripts\empacotar.ps1" }
$Origem = Resolve-Path $Origem
$Exe = Join-Path $Origem "reflex.exe"
if (-not (Test-Path $Exe)) { throw "Monte o pacote antes: .\scripts\empacotar.ps1" }

# Reusa a sessão de vm-conectar.ps1, se houver (não pede a senha de novo).
$reusada = $global:ReflexVM -and $global:ReflexVM.State -eq "Opened"
if ($reusada) {
    $s = $global:ReflexVM
    $VMName = $global:ReflexVMName
} else {
    if (-not $VMName) {
        $ligadas = @(Get-VM | Where-Object State -eq Running)
        if ($ligadas.Count -ne 1) { throw "Há $($ligadas.Count) VMs ligadas; informe -VMName." }
        $VMName = $ligadas[0].Name
    }
    $cred = Get-Credential -UserName $User -Message "Senha do usuário '$User' na VM '$VMName'"
    $s = New-PSSession -VMName $VMName -Credential $cred
}
Write-Host "VM: $VMName" -ForegroundColor Cyan
try {
    if ($Logs) {
        Invoke-Command -Session $s {
            foreach ($f in Get-ChildItem "C:\ProgramData\Reflex\logs\*.log" -ErrorAction SilentlyContinue) {
                Write-Host "`n===== $($f.Name) =====" -ForegroundColor Yellow
                Get-Content $f.FullName -Tail 40
            }
        }
        return
    }

    Write-Host "1/5 Parando o Reflex na VM (se houver)..." -ForegroundColor Cyan
    Invoke-Command -Session $s {
        param($p)
        Stop-Service Reflex -Force -ErrorAction SilentlyContinue
        Get-Process reflex -ErrorAction SilentlyContinue | Stop-Process -Force
        New-Item -ItemType Directory -Force $p | Out-Null
    } -ArgumentList $Pasta

    Write-Host "2/5 Copiando reflex.exe e DLLs para $Pasta (só o que mudou)..." -ForegroundColor Cyan
    $locais = Get-ChildItem $Origem -File | Where-Object { $_.Name -eq "reflex.exe" -or $_.Extension -eq ".dll" }
    $remotos = Invoke-Command -Session $s {
        param($p)
        $h = @{}
        Get-ChildItem $p -File -ErrorAction SilentlyContinue | ForEach-Object { $h[$_.Name] = (Get-FileHash $_.FullName -Algorithm SHA256).Hash }
        $h
    } -ArgumentList $Pasta
    foreach ($f in $locais) {
        if ($remotos[$f.Name] -eq (Get-FileHash $f.FullName -Algorithm SHA256).Hash) {
            Write-Host "    igual: $($f.Name)"
        } else {
            Write-Host "    copiando: $($f.Name)"
            Copy-Item $f.FullName -ToSession $s -Destination $Pasta -Force
        }
    }

    Write-Host "3/5 Instalando o serviço..." -ForegroundColor Cyan
    Invoke-Command -Session $s { param($p) [Console]::OutputEncoding = [Text.Encoding]::UTF8; & "$p\reflex.exe" --install } -ArgumentList $Pasta

    Write-Host "4/5 Cadastrando este PC como master..." -ForegroundColor Cyan
    $meuId = if ($ViewerId) { $ViewerId } else { (& $Exe --my-id).Trim() }
    Write-Host "    ID do viewer deste PC: $meuId"
    # Rodar de novo avisa "já cadastrado"/"já tem master" — sem problema
    # (depois do 1º master, só um master conectado cadastra outro).
    Invoke-Command -Session $s {
        param($p, $id, $n)
        [Console]::OutputEncoding = [Text.Encoding]::UTF8
        $ErrorActionPreference = "Continue"
        & "$p\reflex.exe" --add-user $id $n master 2>&1 | ForEach-Object { "    $_" }
    } -ArgumentList $Pasta, $meuId, $Nome

    Write-Host "5/5 Estado na VM:" -ForegroundColor Cyan
    Invoke-Command -Session $s { param($p) [Console]::OutputEncoding = [Text.Encoding]::UTF8; & "$p\reflex.exe" --status } -ArgumentList $Pasta

    $ips = @((Get-VMNetworkAdapter -VMName $VMName).IPAddresses | Where-Object { $_ -match '^\d+\.\d+\.\d+\.\d+$' -and $_ -notlike '169.254.*' })
    Write-Host ""
    if ($ips.Count -gt 0) {
        Write-Host "Pronto. No Reflex deste PC, aba Conectar, use: $($ips[0]):47800" -ForegroundColor Green
    } else {
        Write-Host "Pronto. O Hyper-V não informou o IP da VM; use um dos endereços do --status acima." -ForegroundColor Green
    }
}
finally {
    if (-not $reusada) { Remove-PSSession $s }
}
