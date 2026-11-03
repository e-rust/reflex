# Roda DENTRO da VM (via scripts\vm-exec.ps1): estado do Reflex.
"--- serviço ---"
Get-Service Reflex -ErrorAction SilentlyContinue | ForEach-Object { "{0}: {1} ({2})" -f $_.Name, $_.Status, $_.StartType }
"--- processos reflex (sessão / linha de comando) ---"
Get-CimInstance Win32_Process -Filter "Name='reflex.exe'" | ForEach-Object {
    $dono = Invoke-CimMethod -InputObject $_ -MethodName GetOwner
    "pid {0,6}  sessão {1}  {2}\{3}  {4}" -f $_.ProcessId, $_.SessionId, $dono.Domain, $dono.User, $_.CommandLine
}
"--- sessões ---"
query session 2>&1
"--- porta 47800 ---"
Get-NetUDPEndpoint -LocalPort 47800 -ErrorAction SilentlyContinue | ForEach-Object { "UDP {0}:{1} pid {2}" -f $_.LocalAddress, $_.LocalPort, $_.OwningProcess }
Get-NetTCPConnection -LocalPort 47800 -State Listen -ErrorAction SilentlyContinue | ForEach-Object { "TCP {0}:{1} pid {2}" -f $_.LocalAddress, $_.LocalPort, $_.OwningProcess }
"--- placa de vídeo ---"
Get-CimInstance Win32_VideoController | ForEach-Object { "{0} ({1}x{2})" -f $_.Name, $_.CurrentHorizontalResolution, $_.CurrentVerticalResolution }
"--- logs ---"
Get-ChildItem C:\ProgramData\Reflex\logs -ErrorAction SilentlyContinue | ForEach-Object { "===== $($_.Name)"; Get-Content $_.FullName -Tail 25 }
