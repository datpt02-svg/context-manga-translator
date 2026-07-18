"""AnyText2 render service — patch embedding_manager for empty font hints."""

from __future__ import annotations

import base64
import os
import sys
import traceback

import cv2
import numpy as np
import torch
from fastapi import FastAPI, HTTPException

try:
    import pkg_resources  # noqa: F401
except ModuleNotFoundError:
    import types as _t
    _pkg_res = _t.ModuleType("pkg_resources")
    _pkg_res.declare_namespace = lambda ns: None
    sys.modules["pkg_resources"] = _pkg_res

_script_dir = os.path.dirname(os.path.abspath(__file__))
_anytext2_repo = os.environ.get("ANYTEXT2_REPO_DIR")
if not _anytext2_repo:
    _candidate = os.path.join(
        os.path.dirname(os.path.dirname(os.path.dirname(_script_dir))), "anytext2")
    if os.path.isdir(_candidate):
        _anytext2_repo = _candidate
if _anytext2_repo and os.path.isdir(_anytext2_repo):
    sys.path.insert(0, _anytext2_repo)
    print(f"[anytext2] using repo at {_anytext2_repo}")

from fastapi.middleware.cors import CORSMiddleware
from schemas import HealthResponse, RenderRequest, RenderResponse, RenderedBlock

app = FastAPI(title="AnyText2 Renderer", version="0.1.0")
app.add_middleware(CORSMiddleware, allow_origins=["*"], allow_methods=["*"], allow_headers=["*"], allow_credentials=True)

MODEL_DIR = (os.environ.get("ANYTEXT2_MODEL_DIR") or
             os.path.join(os.path.dirname(os.path.abspath(__file__)), "models"))
os.environ.setdefault("TRANSFORMERS_CACHE", os.path.join(MODEL_DIR, "hub"))
os.environ.setdefault("HUGGINGFACE_HUB_CACHE", os.path.join(MODEL_DIR, "hub"))
FONT_PATH = os.environ.get("ANYTEXT2_FONT_PATH", "")
for c in [FONT_PATH, "C:/Windows/Fonts/arial.ttf", "C:/Windows/Fonts/Arial.ttf",
          "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf"]:
    if c and os.path.isfile(c):
        FONT_PATH = c
        break
if not FONT_PATH or not os.path.isfile(FONT_PATH):
    print("[anytext2] WARNING: no TrueType font found; set ANYTEXT2_FONT_PATH")
    FONT_PATH = "font/Arial_Unicode.ttf"
MODEL_PATH = os.environ.get("ANYTEXT2_MODEL_PATH", "models/anytext_v2.0.ckpt")
DEVICE = os.environ.get("ANYTEXT2_DEVICE", "cuda" if torch.cuda.is_available() else "cpu")
USE_FP16 = os.environ.get("ANYTEXT2_FP16", "0") == "1"

_inference: object | None = None
_model_loaded = False


def load_model() -> None:
    global _inference, _model_loaded
    if _model_loaded:
        return
    if DEVICE.startswith("cuda") and not torch.cuda.is_available():
        sys.exit("FATAL: CUDA not available.")
    print(f"[anytext2] Loading model on {DEVICE} (fp16={USE_FP16})")
    try:
        from ms_wrapper import AnyText2Model
        _inference = AnyText2Model(
            model_dir=MODEL_DIR, use_fp16=USE_FP16,
            use_translator=False, font_path=FONT_PATH, model_path=MODEL_PATH,
        ).to(DEVICE).eval()

        # Patch encode_text to survive empty font_hint_mimic_imgs
        em = _inference.model.embedding_manager
        _orig_enc = em.encode_text

        def _patched_enc(self_, text_info):
            if self_.font_hint_mimic_imgs is None:
                self_.font_hint_mimic_imgs = []
            n = max(1, len(text_info.get("n_lines", [1])))
            while len(self_.font_hint_mimic_imgs) < n:
                self_.font_hint_mimic_imgs.append([])
            for i in range(n):
                want = text_info["n_lines"][i] if i < len(text_info["n_lines"]) else 1
                while len(self_.font_hint_mimic_imgs[i]) < want:
                    self_.font_hint_mimic_imgs[i].append(None)
            return _orig_enc(text_info)

        em.encode_text = _patched_enc.__get__(em, type(em))
        _model_loaded = True
        print("[anytext2] Model loaded")
    except Exception as exc:
        print(f"[anytext2] Load failed: {exc}")
        traceback.print_exc()
        raise


def _decode_base64(b64: str) -> np.ndarray:
    raw = base64.b64decode(b64)
    img = cv2.imdecode(np.frombuffer(raw, dtype=np.uint8), cv2.IMREAD_COLOR)
    if img is None:
        raise ValueError("Failed to decode image")
    return cv2.cvtColor(img, cv2.COLOR_BGR2RGB)


def _decode_mask_base64(b64: str) -> np.ndarray | None:
    if not b64:
        return None
    raw = base64.b64decode(b64)
    img = cv2.imdecode(np.frombuffer(raw, dtype=np.uint8), cv2.IMREAD_GRAYSCALE)
    return img


def _encode_base64(img: np.ndarray) -> str:
    success, buf = cv2.imencode(".png", cv2.cvtColor(img, cv2.COLOR_RGB2BGR))
    if not success:
        raise ValueError("Failed to encode image")
    return base64.b64encode(buf.tobytes()).decode("utf-8")


def _render_block(source_crop: np.ndarray, mask_crop: np.ndarray,
                  translation: str, text_color: list[int]) -> np.ndarray:
    global _inference
    h, w = source_crop.shape[:2]
    draw_pos = mask_crop
    if draw_pos.ndim == 2:
        draw_pos = np.stack([draw_pos] * 3, axis=-1)
    input_data = {
        "img_prompt": "", "text_prompt": f'"{translation}"', "seed": 42,
        "draw_pos": draw_pos, "ori_image": source_crop,
    }
    params = {
        "mode": "edit", "image_count": 1, "ddim_steps": 10,
        "image_width": w, "image_height": h, "strength": 0.4,
        "cfg_scale": 7.5,
        "text_colors": f"{text_color[0]},{text_color[1]},{text_color[2]}",
        "font_hint_image": [], "font_hint_mask": [],
    }
    results, code, warning_msg, _ = _inference(input_data, **params)
    if code != 0:
        print(f"[anytext2] Warning: {warning_msg}")
    return results[0] if results and len(results) > 0 else source_crop


@app.on_event("startup")
async def startup() -> None:
    load_model()


@app.get("/health")
async def health():
    return {"ok": True, "model_loaded": _model_loaded, "device": DEVICE}


@app.post("/render", response_model=RenderResponse)
async def render(req: RenderRequest) -> RenderResponse:
    if not _model_loaded:
        raise HTTPException(status_code=503, detail="Model not loaded")
    warnings: list[str] = []
    rendered_blocks: list[RenderedBlock] = []

    for block in req.blocks:
        t = block.translation.strip()
        if not t:
            warnings.append(f"Block {block.id}: empty translation, skipping")
            continue
        try:
            source_crop = _decode_base64(block.sourceCropBase64)
            mask_crop = _decode_mask_base64(block.maskCropBase64)
        except ValueError as exc:
            warnings.append(f"Block {block.id}: decode error — {exc}")
            continue

        if mask_crop is None:
            mask_crop = np.zeros((source_crop.shape[0], source_crop.shape[1]), dtype=np.uint8)
        elif source_crop.shape[:2] != mask_crop.shape[:2]:
            mask_crop = cv2.resize(mask_crop, (source_crop.shape[1], source_crop.shape[0]),
                                   interpolation=cv2.INTER_NEAREST)

        h, w = source_crop.shape[:2]
        pad_h, pad_w = max(0, 64 - h), max(0, 64 - w)
        if pad_h or pad_w:
            source_crop = cv2.copyMakeBorder(source_crop, 0, pad_h, 0, pad_w, cv2.BORDER_REPLICATE)
            mask_crop = cv2.copyMakeBorder(mask_crop, 0, pad_h, 0, pad_w, cv2.BORDER_REPLICATE)

        try:
            result = _render_block(source_crop, mask_crop, t, block.textColor)
        except Exception as exc:
            warnings.append(f"Block {block.id}: render error — {exc}")
            traceback.print_exc()
            continue

        if pad_h or pad_w:
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
