"""Unlimited-OCR HTTP service.

Loads baidu/Unlimited-OCR once at startup and exposes:
  GET  /health
  POST /ocr/crops
"""

from __future__ import annotations

import base64
import io
import os
import sys
import traceback
import warnings as _warnings
from typing import Any

import torch
from fastapi import FastAPI, HTTPException
from PIL import Image

from schemas import CropImage, HealthResponse, OcrCropsRequest, OcrCropsResponse, OcrItem

app = FastAPI(title="Unlimited-OCR", version="0.1.0")

# ---------------------------------------------------------------------------
# Configuration from env
# ---------------------------------------------------------------------------
MODEL_NAME = os.environ.get("UNLIMITED_OCR_MODEL", "baidu/Unlimited-OCR")
DEVICE = os.environ.get("UNLIMITED_OCR_DEVICE", "cuda" if torch.cuda.is_available() else "cpu")
MAX_CROPS = int(os.environ.get("MAX_CROPS_PER_REQUEST", "64"))
MAX_SIDE = int(os.environ.get("MAX_IMAGE_SIDE", "1024"))

# ---------------------------------------------------------------------------
# Global model reference (lazy-loaded at startup)
# ---------------------------------------------------------------------------
_model: Any = None
_model_loaded = False


def load_model() -> None:
    """Load the Unlimited-OCR model. Called once during startup."""
    global _model, _model_loaded
    if _model_loaded:
        return

    if DEVICE.startswith("cuda") and not torch.cuda.is_available():
        sys.exit(
            "FATAL: UNLIMITED_OCR_DEVICE=cuda but CUDA is not available. "
            "Set UNLIMITED_OCR_DEVICE=cpu or install a CUDA-compatible torch."
        )

    print(f"[unlimited-ocr] Loading model: {MODEL_NAME} on {DEVICE}")
    try:
        from transformers import AutoModel, AutoTokenizer

        tokenizer = AutoTokenizer.from_pretrained(MODEL_NAME, trust_remote_code=True)
        model = AutoModel.from_pretrained(
            MODEL_NAME,
            trust_remote_code=True,
            torch_dtype=torch.float16 if DEVICE.startswith("cuda") else torch.float32,
        ).to(DEVICE)
        model.eval()
        _model = {"model": model, "tokenizer": tokenizer}
        _model_loaded = True
        print(f"[unlimited-ocr] Model loaded on {DEVICE}")
    except Exception as exc:
        print(f"[unlimited-ocr] Failed to load model: {exc}")
        raise


@app.on_event("startup")
async def startup() -> None:
    load_model()


@app.get("/health", response_model=HealthResponse)
async def health() -> HealthResponse:
    return HealthResponse(
        ok=True,
        model_loaded=_model_loaded,
        device=DEVICE,
        model=MODEL_NAME,
    )


@app.post("/ocr/crops", response_model=OcrCropsResponse)
async def ocr_crops(req: OcrCropsRequest) -> OcrCropsResponse:
    if not _model_loaded:
        raise HTTPException(status_code=503, detail="Model not loaded")

    items: list[OcrItem] = []
    warnings: list[str] = []
    images: list[Image.Image] = []

    # Decode images
    for crop in req.images[:MAX_CROPS]:
        try:
            raw = base64.b64decode(crop.image_base64)
            img = Image.open(io.BytesIO(raw)).convert("RGB")
            # Resize if necessary
            w, h = img.size
            if w > MAX_SIDE or h > MAX_SIDE:
                ratio = MAX_SIDE / max(w, h)
                img = img.resize((int(w * ratio), int(h * ratio)), Image.LANCZOS)
            images.append(img)
        except Exception as exc:
            warnings.append(f"Failed to decode image {crop.id}: {exc}")
            items.append(OcrItem(id=crop.id, text="", uncertain=True))

    if not images:
        return OcrCropsResponse(items=items, warnings=warnings)

    # Run inference
    try:
        outputs = _run_inference(images, req.language_hint)
    except Exception as exc:
        traceback.print_exc()
        raise HTTPException(status_code=500, detail=str(exc))

    # Map results back to ids (text-only crops come first)
    text_ids = [c.id for c in req.images[:MAX_CROPS] if _decode_ok(c)]
    for i, (img_id, out) in enumerate(zip(text_ids, outputs)):
        try:
            text = str(out).strip()
            items.append(
                OcrItem(
                    id=img_id,
                    text=text,
                    confidence=None,
                    uncertain=not text or len(text) <= 1,
                )
            )
        except Exception as exc:
            warnings.append(f"Failed to parse output for {img_id}: {exc}")
            items.append(OcrItem(id=img_id, text="", uncertain=True))

    return OcrCropsResponse(items=items, warnings=warnings)


def _decode_ok(crop: CropImage) -> bool:
    """Quick check whether a base64 image is likely decodable."""
    try:
        base64.b64decode(crop.image_base64)
        return True
    except Exception:
        return False


def _run_inference(images: list[Image.Image], language_hint: str) -> list[str]:
    """Run Unlimited-OCR on a batch of images and return a list of texts."""
    model = _model["model"]
    tokenizer = _model["tokenizer"]

    # Avoid importing here if it's already done at the top
    with _warnings.catch_warnings():
        _warnings.simplefilter("ignore")
        results = model.generate(
            images,
            tokenizer=tokenizer,
            language=language_hint,
        )

    # The model returns one string per image; handle both list and single returns
    if isinstance(results, str):
        return [results]
    return list(results)


if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("PORT", "7862"))
    uvicorn.run(app, host="127.0.0.1", port=port)
