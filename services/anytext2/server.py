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

from schemas import HealthResponse, RenderRequest, RenderResponse, RenderedBlock

from fastapi.middleware.cors import CORSMiddleware

app = FastAPI(title="AnyText2 Renderer", version="0.1.0")
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

