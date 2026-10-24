# Stops PhotoCraft's cloud AI notebook on Kaggle, which saves the account's weekly GPU hours.
param([string]$AiDir = 'D:\AI')

$Cloud = "$AiDir\cloud"
$serverFile = "$Cloud\server.txt"
$config = Get-Content -Raw "$Cloud\config.json" | ConvertFrom-Json
$url = if (Test-Path $serverFile) { Get-Content $serverFile -TotalCount 1 } else { '' }
if ($url) {
    try {
        Invoke-WebRequest "$url/photocraft/stop" -Method Post -Headers @{ Authorization = "Bearer $($config.token)" } -UseBasicParsing -TimeoutSec 30 | Out-Null
        Write-Host 'The cloud AI server is stopping.'
    } catch { Write-Host 'The cloud AI server was not running.' }
} else { Write-Host 'The cloud AI server was not running.' }
Set-Content -Path $serverFile -Value ''
Start-Sleep 4
