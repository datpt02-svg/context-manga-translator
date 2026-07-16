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

# 2. Download AnyText2 checkpoint via ModelScope
Write-Host "[anytext2] Downloading AnyText2 checkpoint from ModelScope..." -ForegroundColor Cyan
$env:MODELSCOPE_CACHE = $ModelsDir
& uv run python -c "
import os
import shutil
from modelscope import snapshot_download

model_dir = snapshot_download('iic/cv_anytext2')
dst = os.path.join(os.environ.get('MODELSCOPE_CACHE', './models'), 'anytext_v2.0.ckpt')
src = os.path.join(model_dir, 'anytext_v2.0.ckpt')
if os.path.exists(src):
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    shutil.copy2(src, dst)
    print(f'Copied checkpoint to {dst}')
else:
    # Search for any .ckpt file
    for root, dirs, files in os.walk(model_dir):
        for f in files:
            if f.endswith('.ckpt'):
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                shutil.copy2(os.path.join(root, f), dst)
                print(f'Found and copied {f} to {dst}')
                break
        else:
            continue
        break
"
if ($LASTEXITCODE -ne 0) { Write-Host "Model download failed" -ForegroundColor Red; exit 1 }

# 3. Download CLIP tokenizer + model from HF into hub cache
Write-Host "[anytext2] Downloading CLIP tokenizer..." -ForegroundColor Cyan
$env:TRANSFORMERS_CACHE = $HubCache
$env:HUGGINGFACE_HUB_CACHE = $HubCache
& uv run python -c "
from transformers import CLIPTokenizer, CLIPTextModel
CLIPTokenizer.from_pretrained('openai/clip-vit-large-patch14')
CLIPTextModel.from_pretrained('openai/clip-vit-large-patch14')
"
if ($LASTEXITCODE -ne 0) { Write-Host "CLIP download failed" -ForegroundColor Red; exit 1 }

# 4. Symlink / copy CLIP into models/ so ms_wrapper finds it as a local folder
$ClipTarget = Join-Path $ModelsDir "clip-vit-large-patch14"
if (-not (Test-Path $ClipTarget)) {
    # Find the snapshot in HF cache (either hub/ or user profile)
    $CacheDirs = @(
        (Join-Path $HubCache "models--openai--clip-vit-large-patch14\snapshots"),
        (Join-Path $Env:USERPROFILE ".cache\huggingface\hub\models--openai--clip-vit-large-patch14\snapshots")
    )
    $Snap = $null
    foreach ($d in $CacheDirs) {
        $Snap = Get-ChildItem $d -Directory -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($Snap) { break }
    }
    if (-not $Snap) {
        Write-Host "CLIP snapshot not found in HF cache" -ForegroundColor Red
        exit 1
    }
    Write-Host "[anytext2] Copying CLIP snapshot to $ClipTarget ..." -ForegroundColor Cyan
    Copy-Item "$($Snap.FullName)\*" $ClipTarget -Recurse -Force
}

Write-Host "[anytext2] Bootstrap complete! Run: uv run python server.py" -ForegroundColor Green
