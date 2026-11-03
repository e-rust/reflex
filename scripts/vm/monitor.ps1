# Roda DENTRO da VM: estado dos monitores e da energia da tela (pra entender
# o que muda quando a janela do Hyper-V (VMConnect) fecha).
"agora: " + (Get-Date).ToUniversalTime().ToString("HH:mm:ss") + "Z"
"--- monitores (PnP) ---"
Get-PnpDevice -Class Monitor | ForEach-Object { "{0} | {1} | presente={2}" -f $_.FriendlyName, $_.Status, $_.Present }
"--- monitores ativos (WMI) ---"
Get-CimInstance -Namespace root\wmi -ClassName WmiMonitorBasicDisplayParams -ErrorAction SilentlyContinue | ForEach-Object { "{0} ativo={1}" -f $_.InstanceName, $_.Active }
"--- pedidos de energia ---"
powercfg /requests
"--- eventos de vídeo/energia recentes ---"
Get-WinEvent -FilterHashtable @{ LogName = 'System'; StartTime = (Get-Date).AddMinutes(-20) } -ErrorAction SilentlyContinue |
    Where-Object { $_.ProviderName -match 'Kernel-Power|Display|Dxgkrnl|Video|hyperv' } |
    Select-Object -First 15 | ForEach-Object { "{0:HH:mm:ss} {1} {2}: {3}" -f $_.TimeCreated.ToUniversalTime(), $_.ProviderName, $_.Id, ($_.Message -split "`n")[0] }
