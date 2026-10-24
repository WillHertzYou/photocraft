# Installs the AI server for PhotoCraft's AI features (IOPaint + PyTorch for CUDA 12.6) into
# $AiDir, about 8 GB with the models. Needs Python 3.11 and an NVIDIA card with a current driver.
# CUDA 12.6 builds still support Maxwell cards such as the GTX 980 (sm_50/52); newer CUDA builds don't.
param([string]$AiDir = 'D:\AI', [string]$Python = 'py -3.11')

$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $AiDir | Out-Null
Invoke-Expression "$Python -m venv `"$AiDir\venv`""
$py = "$AiDir\venv\Scripts\python.exe"
& $py -m pip install --upgrade pip
& $py -m pip install torch==2.7.1 torchvision==0.22.1 --index-url https://download.pytorch.org/whl/cu126
& $py -m pip install iopaint==1.6.0
# IOPaint's Remove Background plugin refuses to start without rembg. Installed without its own
# dependencies so IOPaint's pins (Pillow 9.5, NumPy 1.26) stay; opencv-python already provides cv2.
& $py -m pip install --no-deps rembg==2.0.59
& $py -m pip install onnxruntime pooch pymatting "numpy==1.26.4" "pillow==9.5.0"
Copy-Item -Force "$PSScriptRoot\start-ai-server.ps1" "$AiDir\start-ai-server.ps1"

# Download the models now rather than on first use.
$env:CUDA_VISIBLE_DEVICES = '0'
$env:HF_HOME = "$AiDir\models\huggingface"
$env:TORCH_HOME = "$AiDir\models\torch"
& "$AiDir\venv\Scripts\iopaint.exe" download --model lama --model-dir "$AiDir\models"
& "$AiDir\venv\Scripts\iopaint.exe" download --model Uminosachi/realisticVisionV51_v51VAE-inpainting --model-dir "$AiDir\models"
Write-Host "Done. Start the server with: powershell -ExecutionPolicy Bypass -File `"$AiDir\start-ai-server.ps1`""
