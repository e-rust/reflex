# Roda DENTRO da VM: bloqueia a sessão do console (como Win+L), por uma tarefa
# agendada que executa na sessão interativa do usuário.
$usuario = "claude"
$acao = New-ScheduledTaskAction -Execute "rundll32.exe" -Argument "user32.dll,LockWorkStation"
$quem = New-ScheduledTaskPrincipal -UserId $usuario -LogonType Interactive
Register-ScheduledTask -TaskName "ReflexBloquear" -Action $acao -Principal $quem -Force | Out-Null
Start-ScheduledTask -TaskName "ReflexBloquear"
Start-Sleep -Seconds 2
Unregister-ScheduledTask -TaskName "ReflexBloquear" -Confirm:$false
"Sessão de $usuario bloqueada."
