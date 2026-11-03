# Roda DENTRO da VM: apaga o monitor (como a economia de energia faz), por uma
# tarefa agendada na sessão interativa do usuário. Serve pra testar se o
# Reflex religa a tela sozinho ao conectar.
$usuario = "claude"
$cmd = 'Add-Type -Namespace W -Name U -MemberDefinition ''[DllImport(\"user32.dll\")] public static extern int PostMessage(int h, int m, int w, int l);''; [W.U]::PostMessage(0xFFFF, 0x0112, 0xF170, 2)'
$acao = New-ScheduledTaskAction -Execute "powershell.exe" -Argument "-NoProfile -WindowStyle Hidden -Command `"$cmd`""
$quem = New-ScheduledTaskPrincipal -UserId $usuario -LogonType Interactive
Register-ScheduledTask -TaskName "ReflexApagarTela" -Action $acao -Principal $quem -Force | Out-Null
Start-ScheduledTask -TaskName "ReflexApagarTela"
Start-Sleep -Seconds 4
Unregister-ScheduledTask -TaskName "ReflexApagarTela" -Confirm:$false
"Monitor da VM apagado."
