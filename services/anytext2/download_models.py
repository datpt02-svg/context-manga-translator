"""Download AnyText2 model checkpoints."""

import os
import sys

try:
    from modelscope import snapshot_download
except ImportError:
    sys.exit("Install modelscope first: pip install modelscope")

MODEL_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "models")
os.makedirs(MODEL_DIR, exist_ok=True)

cache_dir = snapshot_download("iic/cv_anytext2")
print(f"Downloaded to cache: {cache_dir}")

# Copy checkpoint + CLIP into local models/
import shutil

src = cache_dir
for item in ["anytext_v2.0.ckpt", "clip-vit-large-patch14"]:
    s = os.path.join(src, item)
    d = os.path.join(MODEL_DIR, item)
    if os.path.isfile(s):
        shutil.copy2(s, d)
        print(f"  Copied {item}")
    elif os.path.isdir(s):
        if os.path.exists(d):
            shutil.rmtree(d)
        shutil.copytree(s, d)
        print(f"  Copied {item}/")

print(f"Done. Models ready at {MODEL_DIR}")
