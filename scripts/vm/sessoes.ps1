# Roda DENTRO da VM: processos do Reflex e o fim do log do agente, com as
# medidas do motor (pra ver o que acontece ao conectar/desconectar).
"agora: " + (Get-Date).ToUniversalTime().ToString("HH:mm:ss") + "Z"
"--- processos do Reflex ---"
Get-CimInstance Win32_Process -Filter "Name='reflex.exe'" | ForEach-Object { "pid {0} sessão {1} {2}" -f $_.ProcessId, $_.SessionId, $_.CommandLine }
"--- vídeo ---"
Get-CimInstance Win32_VideoController | ForEach-Object { "{0} {1}x{2} {3}" -f $_.Name, $_.CurrentHorizontalResolution, $_.CurrentVerticalResolution, $_.Status }
"--- agent.log ---"
Get-Content C:\ProgramData\Reflex\logs\agent.log -Tail 30
