# Sets up PhotoCraft's cloud AI on this PC: the Kaggle command-line tool, a private token and
# ntfy.sh topic for the notebook, PHOTOCRAFT_AI_CLOUD_FILE for PhotoCraft, and desktop shortcuts
# Start Cloud AI and Stop Cloud AI. Needs Python 3.11 and a Kaggle API token in
# %USERPROFILE%\.kaggle\kaggle.json (kaggle.com > Settings > API > Create New Token); the account
# must be phone-verified for notebooks to use the internet.
param([string]$AiDir = 'D:\AI', [string]$Python = 'py -3.11')

$ErrorActionPreference = 'Stop'
$Cloud = "$AiDir\cloud"
New-Item -ItemType Directory -Force -Path $Cloud | Out-Null
foreach ($f in 'photocraft_ai_kaggle.py', 'start-cloud-ai.ps1', 'stop-cloud-ai.ps1') {
    Copy-Item -Force "$PSScriptRoot\$f" "$Cloud\$f"
}

if (-not (Test-Path "$AiDir\kaggle-cli\Scripts\kaggle.exe")) {
    Invoke-Expression "$Python -m venv `"$AiDir\kaggle-cli`""
    & "$AiDir\kaggle-cli\Scripts\python.exe" -m pip install -q --upgrade pip kaggle
}

$kaggleJson = "$env:USERPROFILE\.kaggle\kaggle.json"
if (-not (Test-Path $kaggleJson)) { throw "No Kaggle API token: save kaggle.json to $kaggleJson first." }
$kaggleUser = (Get-Content -Raw $kaggleJson | ConvertFrom-Json).username

# The token only this PC and its notebook know, and a topic name nobody else will guess.
$configPath = "$Cloud\config.json"
if (Test-Path $configPath) {
    $config = Get-Content -Raw $configPath | ConvertFrom-Json
} else {
    $bytes = New-Object byte[] 44
    [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    $hex = -join ($bytes | ForEach-Object { $_.ToString('x2') })
    $config = [pscustomobject]@{ token = $hex.Substring(0, 64); topic = "photocraft-ai-$($hex.Substring(64, 24))" }
}
$config | Add-Member -Force -NotePropertyName kaggleUser -NotePropertyValue $kaggleUser
[IO.File]::WriteAllText($configPath, ($config | ConvertTo-Json))
if (-not (Test-Path "$Cloud\server.txt")) { Set-Content -Path "$Cloud\server.txt" -Value '' }

# PhotoCraft sends prompted fills to the server named in this file (empty while it is stopped).
[Environment]::SetEnvironmentVariable('PHOTOCRAFT_AI_CLOUD_FILE', "$Cloud\server.txt", 'User')

$shell = New-Object -ComObject WScript.Shell
$desktop = [Environment]::GetFolderPath('Desktop')
foreach ($s in @(@('Start Cloud AI', 'start-cloud-ai.ps1'), @('Stop Cloud AI', 'stop-cloud-ai.ps1'))) {
    $lnk = $shell.CreateShortcut("$desktop\$($s[0]).lnk")
    $lnk.TargetPath = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
    $lnk.Arguments = "-NoProfile -ExecutionPolicy Bypass -File `"$Cloud\$($s[1])`" -AiDir `"$AiDir`""
    $lnk.WorkingDirectory = $Cloud
    $lnk.Save()
}
Write-Host "Done (Kaggle user $kaggleUser). Restart PhotoCraft once so it sees PHOTOCRAFT_AI_CLOUD_FILE."
