# Unlimited-OCR Service

HTTP service wrapping [baidu/Unlimited-OCR](https://huggingface.co/baidu/Unlimited-OCR).

## Quick Start

```powershell
cd services/unlimited-ocr
./run.ps1
```

```bash
cd services/unlimited-ocr
./run.sh
```

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `UNLIMITED_OCR_MODEL` | `baidu/Unlimited-OCR` | HuggingFace model ID |
| `UNLIMITED_OCR_DEVICE` | `cuda` (if available) / `cpu` | Device for inference |
| `MAX_CROPS_PER_REQUEST` | `64` | Max image crops per request |
| `MAX_IMAGE_SIDE` | `1024` | Max image side length (px) |
| `PORT` | `7862` | HTTP port |

## API

### GET /health

```json
{"ok": true, "modelLoaded": true, "device": "cuda", "model": "baidu/Unlimited-OCR"}
```

### POST /ocr/crops

Request:
```json
{"images": [{"id": "node-id", "imageBase64": "<png-base64>"}], "languageHint": "ja", "returnContext": true}
```

Response:
```json
{"items": [{"id": "node-id", "text": "何してるの？", "confidence": null, "uncertain": false, ...}], "pageContext": null, "warnings": []}
```
