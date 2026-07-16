"""AnyText2 render service.

Inference endpoint for AnyText2 (https://github.com/tyxsspa/anytext2).
Loads the model once at startup; per-block editing via POST /render.
"""

from __future__ import annotations

import base64
import io
import os
import sys
import traceback

import cv2
import numpy as np
import torch
from fastapi import FastAPI, HTTPException
from PIL import Image

from schemas import (
    FontHint,
    HealthResponse,
    RenderRequest,
    RenderResponse,
    RenderedBlock,
)

app = FastAPI(title="AnyText2 Renderer", version="0.1.0")

# ---------------------------------------------------------------------------
# Configuration from env
# ---------------------------------------------------------------------------
MODEL_DIR = os.environ.get("ANYTEXT2_MODEL_DIR", "./models")
FONT_PATH = os.environ.get("ANYTEXT2_FONT_PATH", "font/Arial_Unicode.ttf")
MODEL_PATH = os.environ.get("ANYTEXT2_MODEL_PATH", "models/anytext_v2.0.ckpt")
DEVICE = os.environ.get("ANYTEXT2_DEVICE", "cuda" if torch.cuda.is_available() else "cpu")
USE_FP16 = os.environ.get("ANYTEXT2_FP16", "1") == "1"
MAX_IMAGE_SIDE = int(os.environ.get("ANYTEXT2_MAX_SIDE", "2048"))

# ---------------------------------------------------------------------------
# Global model reference
# ---------------------------------------------------------------------------
_inference: object | None = None
_model_loaded = False


def load_model() -> None:
    """Load AnyText2 model. Called once during startup."""
    global _inference, _model_loaded
    if _model_loaded:
        return

    if DEVICE.startswith("cuda") and not torch.cuda.is_available():
        sys.exit(
            "FATAL: ANYTEXT2_DEVICE=cuda but CUDA is not available. "
            "Set ANYTEXT2_DEVICE=cpu or install a CUDA-compatible torch."
        )

    print(f"[anytext2] Loading model from {MODEL_DIR} on {DEVICE} (fp16={USE_FP16})")
    try:
        from ms_wrapper import AnyText2Model

        _inference = (
            AnyText2Model(
                model_dir=MODEL_DIR,
                use_fp16=USE_FP16,
                use_translator=False,
                font_path=FONT_PATH,
                model_path=MODEL_PATH,
            )
            .to(DEVICE)
            .eval()
        )
        _model_loaded = True
        print(f"[anytext2] Model loaded on {DEVICE}")
    except ImportError:
        print(
            "[anytext2] ms_wrapper not installed. Install with: "
            "pip install ms_wrapper  (see https://github.com/tyxsspa/anytext2)"
        )
        raise
    except Exception as exc:
        print(f"[anytext2] Failed to load model: {exc}")
        traceback.print_exc()
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
    )


def _decode_base64(b64: str) -> np.ndarray:
    """Decode a base64 PNG/JPG string to an RGB numpy array."""
    raw = base64.b64decode(b64)
    arr = np.frombuffer(raw, dtype=np.uint8)
    img = cv2.imdecode(arr, cv2.IMREAD_COLOR)
    if img is None:
        raise ValueError("Failed to decode image from base64")
    return cv2.cvtColor(img, cv2.COLOR_BGR2RGB)


def _encode_base64(img: np.ndarray) -> str:
    """Encode an RGB numpy array (H×W×3) to a base64 PNG string."""
    success, buf = cv2.imencode(".png", cv2.cvtColor(img, cv2.COLOR_RGB2BGR))
    if not success:
        raise ValueError("Failed to encode image to PNG")
    return base64.b64encode(buf.tobytes()).decode("utf-8")


def _render_block(
    inpainted_crop: np.ndarray,
    source_crop: np.ndarray,
    translation: str,
    text_color: list[int],
) -> np.ndarray:
    """Run AnyText2 mode='edit' on a single crop and return the result."""
    global _inference
    if _inference is None:
        raise RuntimeError("Model not loaded")

    h, w = inpainted_crop.shape[:2]

    # draw_pos: white text region on black background — here we mark the
    # entire crop area so AnyText2 places the text centred in this region.
    draw_pos = np.zeros((h, w, 3), dtype=np.uint8)
    # Shrink by 8px on each side so text doesn't touch the border.
    margin = 8
    if w > margin * 2 and h > margin * 2:
        draw_pos[margin : h - margin, margin : w - margin] = (255, 255, 255)

    input_data = {
        "img_prompt": "",
        "text_prompt": f'"{translation}"',
        "seed": 42,  # fixed seed for deterministic per-session output
        "draw_pos": 255 - draw_pos,
        "ori_image": inpainted_crop,
    }

    params = {
        "mode": "edit",
        "image_count": 1,
        "ddim_steps": 20,
        "image_width": w,
        "image_height": h,
        "strength": 0.8,
        "cfg_scale": 7.5,
        "text_colors": f"{text_color[0]},{text_color[1]},{text_color[2]}",
    }

    results, code, warning_msg, debug_info = _inference(input_data, **params)  # type: ignore[misc]
    if code != 0:
        print(f"[anytext2] Warning from model: {warning_msg}")

    if results and len(results) > 0:
        return results[0]
    # Fallback: return the input crop unchanged
    return inpainted_crop


@app.post("/render", response_model=RenderResponse)
async def render(req: RenderRequest) -> RenderResponse:
    if not _model_loaded or _inference is None:
        raise HTTPException(status_code=503, detail="Model not loaded")

    warnings: list[str] = []
    rendered_blocks: list[RenderedBlock] = []

    for block in req.blocks:
        translation = block.translation.strip()
        if not translation:
            warnings.append(f"Block {block.id}: empty translation, skipping")
            continue

        try:
            inpainted_crop = _decode_base64(block.inpaintedCropBase64)
            source_crop = _decode_base64(block.sourceCropBase64)
        except ValueError as exc:
            warnings.append(f"Block {block.id}: decode error — {exc}")
            continue

        # Pad crop to the minimum size AnyText2 handles well
        h, w = inpainted_crop.shape[:2]
        min_side = 64
        pad_h = max(0, min_side - h)
        pad_w = max(0, min_side - w)
        if pad_h > 0 or pad_w > 0:
            inpainted_crop = cv2.copyMakeBorder(
                inpainted_crop, 0, pad_h, 0, pad_w, cv2.BORDER_REPLICATE
            )
            source_crop = cv2.copyMakeBorder(
                source_crop, 0, pad_h, 0, pad_w, cv2.BORDER_REPLICATE
            )

        try:
            result = _render_block(
                inpainted_crop,
                source_crop,
                translation,
                block.textColor,
            )
        except Exception as exc:
            warnings.append(f"Block {block.id}: render error — {exc}")
            traceback.print_exc()
            continue

        # Crop back to original size if we padded
        if pad_h > 0 or pad_w > 0:
            result = result[:h, :w]

        try:
            b64 = _encode_base64(result)
        except ValueError as exc:
            warnings.append(f"Block {block.id}: encode error — {exc}")
            continue

        rendered_blocks.append(RenderedBlock(id=block.id, renderedCropBase64=b64))

    return RenderResponse(blocks=rendered_blocks, warnings=warnings)


if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("PORT", "7863"))
    uvicorn.run(app, host="127.0.0.1", port=port)
