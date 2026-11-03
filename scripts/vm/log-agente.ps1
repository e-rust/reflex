# Roda DENTRO da VM: o que importa do log do agente (sem as linhas repetidas).
$log = "C:\ProgramData\Reflex\logs\agent.log"
"linhas no log: {0}" -f (Get-Content $log).Count
"--- trocas de área de trabalho / erros ---"
Select-String -Path $log -Pattern "área de trabalho|SetThreadDesktop|WARN|ERROR|pânico|motor|sessão|encoder|dispositivo|adaptador" | Select-Object -Last 25 | ForEach-Object { $_.Line }
"--- contagem de recriações (encoder aberto) ---"
(Select-String -Path $log -Pattern "encoder libx264 1024x768").Count
"--- processo do agente ---"
Get-CimInstance Win32_Process -Filter "Name='reflex.exe'" | ForEach-Object { "pid {0} sessão {1} {2}" -f $_.ProcessId, $_.SessionId, $_.CommandLine }
