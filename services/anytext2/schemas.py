"""Pydantic schemas for the AnyText2 render service."""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel


class FontHint(BaseModel):
    serif: bool = False
    language: str | None = None
    family: str | None = None
    fontSizePx: float | None = None


class TextBlock(BaseModel):
    id: str
    translation: str
    x: float
    y: float
    width: float
    height: float
    sourceCropBase64: str
    inpaintedCropBase64: str
    textColor: list[int] = [0, 0, 0, 255]
    fontHint: FontHint | None = None


class RenderRequest(BaseModel):
    imageWidth: int
    imageHeight: int
    sourceImageBase64: str
    inpaintedImageBase64: str
    blocks: list[TextBlock]


class RenderedBlock(BaseModel):
    id: str
    renderedCropBase64: str
    """RGB or RGBA PNG as base64, same dimensions as the input crop."""


class RenderResponse(BaseModel):
    blocks: list[RenderedBlock]
    warnings: list[str] = []


class HealthResponse(BaseModel):
    ok: bool
    model_loaded: bool
    device: str
