# AnyText2 service bootstrap
# Downloads model weights and CLIP tokenizer to the local models/ directory.
# Run once after clone.

$ServiceDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ModelsDir = Join-Path $ServiceDir "models"
$HubCache  = Join-Path $ModelsDir "hub"

Write-Host "[anytext2] Bootstrapping model files..." -ForegroundColor Cyan

# 1. uv sync
Write-Host "[anytext2] uv sync..." -ForegroundColor Cyan
& uv sync
if ($LASTEXITCODE -ne 0) { Write-Host "uv sync failed" -ForegroundColor Red; exit 1 }

# 2. Download CLIP tokenizer + model from HF into hub cache
Write-Host "[anytext2] Downloading CLIP tokenizer..." -ForegroundColor Cyan
$env:TRANSFORMERS_CACHE = $HubCache
$env:HUGGINGFACE_HUB_CACHE = $HubCache
& uv run python -c "
from transformers import CLIPTokenizer, CLIPTextModel
CLIPTokenizer.from_pretrained('openai/clip-vit-large-patch14')
CLIPTextModel.from_pretrained('openai/clip-vit-large-patch14')
"
if ($LASTEXITCODE -ne 0) { Write-Host "CLIP download failed" -ForegroundColor Red; exit 1 }

# 3. Symlink / copy CLIP into models/ so ms_wrapper finds it as a local folder
$ClipTarget = Join-Path $ModelsDir "clip-vit-large-patch14"
if (-not (Test-Path $ClipTarget)) {
    # Find the snapshot in HF cache
    $CacheDir = Join-Path $Env:USERPROFILE ".cache\huggingface\hub\models--openai--clip-vit-large-patch14\snapshots"
    $Snap = Get-ChildItem $CacheDir -Directory | Select-Object -First 1
    if (-not $Snap) {
        Write-Host "CLIP snapshot not found in HF cache" -ForegroundColor Red
        exit 1
    }
    Write-Host "[anytext2] Copying CLIP snapshot to $ClipTarget ..." -ForegroundColor Cyan
    Copy-Item "$($Snap.FullName)\*" $ClipTarget -Recurse -Force
}

Write-Host "[anytext2] Bootstrap complete! Run: uv run python server.py" -ForegroundColor Green
