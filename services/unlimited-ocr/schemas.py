"""Request/response schemas for the Unlimited-OCR service."""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel


class CropImage(BaseModel):
    id: str
    image_base64: str


class OcrCropsRequest(BaseModel):
    images: list[CropImage]
    language_hint: str = "ja"
    return_context: bool = True


class OcrItem(BaseModel):
    id: str
    text: str
    confidence: float | None = None
    uncertain: bool = False
    role: str | None = None
    speaker_hint: str | None = None
    emotion_hint: str | None = None
    visual_hint: str | None = None
    translation_note: str | None = None


class OcrCropsResponse(BaseModel):
    items: list[OcrItem]
    page_context: Any = None
    warnings: list[str] = []


class HealthResponse(BaseModel):
    ok: bool = True
    model_loaded: bool = False
    device: str = "cpu"
    model: str = "baidu/Unlimited-OCR"
