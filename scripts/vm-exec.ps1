#Requires -RunAsAdministrator
<#
.SYNOPSIS
  Roda um arquivo .ps1 DENTRO da VM, pela sessão aberta por vm-conectar.ps1.

    & .\scripts\vm-exec.ps1 .\scripts\vm\diagnostico.ps1
#>
param(
    [Parameter(Mandatory)][string]$Script
)

$ErrorActionPreference = "Stop"
if (-not $global:ReflexVM -or $global:ReflexVM.State -ne "Opened") {
    throw "Sem sessão com a VM. Rode antes: . .\scripts\vm-conectar.ps1"
}
# Lido aqui como UTF-8 (o -FilePath do Invoke-Command não tira o BOM e
# estraga os acentos); vai pra VM como bloco de script.
$codigo = Get-Content -Raw -Encoding UTF8 (Resolve-Path $Script)
Invoke-Command -Session $global:ReflexVM -ScriptBlock ([scriptblock]::Create($codigo))
