# Starts the AI server PhotoCraft uses for Generative Fill and Remove Background:
# IOPaint on http://127.0.0.1:8080, on the NVIDIA card. Models live in $AiDir\models.
# Leave this window open while using the AI features; close it to free the graphics card.
param([string]$AiDir = 'D:\AI')

# Some setups hide GPUs from CUDA (CUDA_VISIBLE_DEVICES=-1); only this server sees the card.
$env:CUDA_VISIBLE_DEVICES = '0'
$env:HF_HOME = "$AiDir\models\huggingface"
$env:TORCH_HOME = "$AiDir\models\torch"
$env:HF_HUB_DISABLE_TELEMETRY = '1'
$Host.UI.RawUI.WindowTitle = 'PhotoCraft AI server (IOPaint)'

& "$AiDir\venv\Scripts\iopaint.exe" start `
    --host 127.0.0.1 --port 8080 `
    --model lama --device cuda --model-dir "$AiDir\models" `
    --low-mem `
    --enable-remove-bg --remove-bg-model briaai/RMBG-1.4 --remove-bg-device cuda
