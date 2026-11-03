#Requires -RunAsAdministrator
<#
.SYNOPSIS
  Abre uma sessão PowerShell Direct com a VM e a guarda em $global:ReflexVM,
  pra os outros scripts (testar-vm.ps1, vm-exec.ps1) reusarem sem pedir a
  senha de novo. Rode com ponto na frente, no mesmo terminal:

    . .\scripts\vm-conectar.ps1
#>
param(
    # Nome da VM; vazio = a única VM ligada.
    [string]$VMName,
    [string]$User = "claude"
)

$ErrorActionPreference = "Stop"
if (-not $VMName) {
    $ligadas = @(Get-VM | Where-Object State -eq Running)
    if ($ligadas.Count -ne 1) { throw "Há $($ligadas.Count) VMs ligadas; informe -VMName." }
    $VMName = $ligadas[0].Name
}
if ($global:ReflexVM) { Remove-PSSession $global:ReflexVM -ErrorAction SilentlyContinue }

$cred = Get-Credential -UserName $User -Message "Senha do usuário '$User' na VM '$VMName'"
$global:ReflexVM = New-PSSession -VMName $VMName -Credential $cred
$global:ReflexVMName = $VMName
$info = Invoke-Command -Session $global:ReflexVM { "$env:COMPUTERNAME | $((Get-CimInstance Win32_OperatingSystem).Caption) | usuário $(whoami)" }
Write-Host "Conectado à VM '$VMName': $info" -ForegroundColor Green
