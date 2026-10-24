# Starts PhotoCraft's cloud AI server (photocraft_ai_kaggle.py) on Kaggle's free GPU, waits until
# it is ready and writes its address to $AiDir\cloud\server.txt, which PhotoCraft reads for each
# prompted Generative Fill (PHOTOCRAFT_AI_CLOUD_FILE). PhotoCraft can stay open meanwhile.
# The notebook stops by itself after -IdleMinutes without a fill; Stop Cloud AI stops it at once.
param([string]$AiDir = 'D:\AI', [int]$IdleMinutes = 30)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$Host.UI.RawUI.WindowTitle = 'PhotoCraft cloud AI (Kaggle)'
$Cloud = "$AiDir\cloud"
$kaggle = "$AiDir\kaggle-cli\Scripts\kaggle.exe"
$serverFile = "$Cloud\server.txt"
$config = Get-Content -Raw "$Cloud\config.json" | ConvertFrom-Json
$auth = @{ Authorization = "Bearer $($config.token)" }

function Test-Server([string]$url) {
    try { Invoke-WebRequest "$url/api/v1/model" -Headers $auth -UseBasicParsing -TimeoutSec 20 | Out-Null; $true } catch { $false }
}

if (Test-Path $serverFile) {
    $url = (Get-Content $serverFile -TotalCount 1)
    if ($url -and (Test-Server $url)) {
        Write-Host "The cloud AI server is already running at $url."
        Start-Sleep 5
        exit 0
    }
}
Set-Content -Path $serverFile -Value ''

# The notebook, with this PC's token and topic filled in.
$push = "$Cloud\push"
New-Item -ItemType Directory -Force -Path $push | Out-Null
$script = Get-Content -Raw "$Cloud\photocraft_ai_kaggle.py"
$script = $script.Replace('__PHOTOCRAFT_TOKEN__', $config.token).Replace('__PHOTOCRAFT_TOPIC__', $config.topic)
$script = $script -replace '(?m)^IDLE_MINUTES = \d+', "IDLE_MINUTES = $IdleMinutes"
[IO.File]::WriteAllText("$push\photocraft_ai_kaggle.py", $script)
$meta = [ordered]@{
    id = "$($config.kaggleUser)/photocraft-cloud-ai"; title = 'photocraft-cloud-ai'
    code_file = 'photocraft_ai_kaggle.py'; language = 'python'; kernel_type = 'script'
    is_private = $true; enable_gpu = $true; enable_internet = $true
    dataset_sources = @(); competition_sources = @(); kernel_sources = @(); model_sources = @()
}
[IO.File]::WriteAllText("$push\kernel-metadata.json", ($meta | ConvertTo-Json))

$since = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() - 5
Write-Host 'Starting the notebook on Kaggle...'
$out = & $kaggle kernels push -p $push --accelerator NvidiaTeslaT4 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) {
    # Older Kaggle CLIs have no --accelerator; Kaggle then picks the GPU.
    $out = & $kaggle kernels push -p $push 2>&1 | Out-String
}
Write-Host $out.Trim()
if ($LASTEXITCODE -ne 0 -or $out -match 'error|invalid|failed') { throw "Kaggle did not start the notebook: $($out.Trim())" }

# Its first start installs IOPaint and downloads the model: about 10 minutes.
Write-Host 'Waiting for it to install and load the model (about 10 minutes the first time)...'
$deadline = (Get-Date).AddMinutes(30)
$status = ''
while ((Get-Date) -lt $deadline) {
    Start-Sleep 10
    try {
        $lines = (Invoke-WebRequest "https://ntfy.sh/$($config.topic)/json?poll=1&since=$since" -UseBasicParsing -TimeoutSec 30).Content -split "`n"
    } catch { continue }
    foreach ($line in $lines | Where-Object { $_.Trim() }) {
        $m = $line | ConvertFrom-Json
        if ($m.event -ne 'message') { continue }
        if ($m.message -ne $status) { $status = $m.message; Write-Host "  notebook: $status" }
        if ($status -like 'failed*') { throw "The notebook failed. Its log is on kaggle.com under Code > photocraft-cloud-ai." }
        if ($status -like 'ready *') {
            $url = $status.Substring(6).Trim()
            if (Test-Server $url) {
                Set-Content -Path $serverFile -Value @($url, $config.token)
                Write-Host "Ready: Generative Fill with a prompt now runs on Kaggle ($url)."
                Write-Host "It stops by itself after $IdleMinutes minutes without a fill."
                Start-Sleep 8
                exit 0
            }
        }
    }
}
throw 'The notebook did not get ready within 30 minutes. Its log is on kaggle.com under Code > photocraft-cloud-ai.'
