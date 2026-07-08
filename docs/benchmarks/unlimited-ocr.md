# Unlimited-OCR Benchmark

## Metrics

Track per-test-run:

| Metric | Description |
|---|---|
| `pages` | Number of pages processed |
| `text_boxes` | Total detected text boxes |
| `base_ocr_time` | Wall-clock for base OCR engine (s) |
| `unlimited_ocr_time` | Wall-clock for Unlimited-OCR calls (s) |
| `fallback_count` | Text boxes re-sent by SmartFallback |
| `total_pipeline_time` | Full pipeline wall-clock (s) |
| `uncertain_before` | `ocrUncertain` count after base OCR |
| `uncertain_after` | `ocrUncertain` count after fallback/Unlimited-OCR |
| `manual_corrections` | User edits needed (estimate) |
| `translation_quality` | Subjective 1-5 (reviewer provides) |

## Modes to benchmark

1. **Off** — base OCR only, no Unlimited-OCR
2. **SmartFallback** — base OCR + suspicious boxes only
3. **Full crop batch** — all text boxes via `/ocr/crops`
4. (future) Full page/contact-sheet via `/ocr/page`

## Prerequisites

```powershell
# Terminal 1 — start Unlimited-OCR service
cd services\unlimited-ocr
.\.venv\Scripts\activate
uvicorn server:app --host 127.0.0.1 --port 7862

# Terminal 2 — set env
$env:UNLIMITED_OCR_URL = "http://127.0.0.1:7862"
```

## Procedure

1. Open a test project in Koharu UI.
2. Run pipeline with mode Off → record metrics.
3. Run pipeline with mode SmartFallback → record metrics.
4. Run pipeline with mode Full → record metrics.
5. Compare uncertain counts and pipeline time.
6. Review translation output quality.

## Sample data

Use only:
- **User-provided** local manga pages (do not commit).
- **Public-domain** or permissively licensed images.
- **Research datasets** whose license permits local testing.

Do **not** commit copyrighted manga samples to the repository.

## Results template

| Run | Pages | Boxes | Base OCR (s) | Unlimited (s) | Fallback | Total (s) | Uncertain Before | Uncertain After | Manual Fixes | Quality (1-5) |
|---|---|---|---|---|---|---|---|---|---|---|
| Off | | | | — | — | | | — | | |
| SmartFallback | | | | | | | | | | |
| Full | | | — | | — | | | | | |

## Notes

- Run each mode 3× and report the median.
- Document the test environment (GPU/CPU model, RAM, OS).
- Note any model errors, warnings, or crashes.
