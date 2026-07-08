# Milestone 2 — OCR Metadata + OCR Quality Checker

## Goal

Add OCR metadata and detect suspicious OCR results.

This milestone still does not call Unlimited-OCR.

---

## Scope

Implement:

1. OCR metadata fields in `TextData`
2. Patch support for metadata
3. OCR quality checker
4. Update existing 3 OCR engines to write metadata

Do not implement:

- Python Unlimited-OCR service
- Rust Unlimited-OCR engine
- Smart fallback mode
- Full Unlimited-OCR mode

---

## Files to inspect

```text
crates/koharu-core/src/scene.rs
crates/koharu-core/src/op.rs
crates/koharu-app/src/pipeline/engines/manga_ocr.rs
crates/koharu-app/src/pipeline/engines/mit48px_ocr.rs
crates/koharu-app/src/pipeline/engines/paddle_ocr.rs
```

---

## Task 1 — Add metadata fields to TextData

File:

```text
crates/koharu-core/src/scene.rs
```

Find `TextData`.

Add after `detector`:

```rust
#[serde(default)]
pub ocr_engine: Option<String>,

#[serde(default)]
pub ocr_confidence: Option<f32>,

#[serde(default)]
pub ocr_uncertain: bool,
```

Meaning:

```text
confidence      = detector/bbox confidence
detector        = text detector model
ocr_engine      = OCR model that wrote current text
ocr_confidence  = OCR score if available
ocr_uncertain   = OCR result looks suspicious
```

Important:

- Do not repurpose existing `confidence`.
- `confidence` remains detector confidence.

---

## Task 2 — Add metadata fields to TextDataPatch

File:

```text
crates/koharu-core/src/op.rs
```

In `TextDataPatch`, add:

```rust
#[serde(default)]
pub ocr_engine: Option<Option<String>>,

#[serde(default)]
pub ocr_confidence: Option<Option<f32>>,

#[serde(default)]
pub ocr_uncertain: Option<bool>,
```

---

## Task 3 — Update capture_prev_text

File:

```text
crates/koharu-core/src/op.rs
```

In `capture_prev_text`, add:

```rust
ocr_engine: p.ocr_engine.as_ref().map(|_| data.ocr_engine.clone()),
ocr_confidence: p.ocr_confidence.as_ref().map(|_| data.ocr_confidence),
ocr_uncertain: p.ocr_uncertain.as_ref().map(|_| data.ocr_uncertain),
```

---

## Task 4 — Update apply_text_patch

File:

```text
crates/koharu-core/src/op.rs
```

In `apply_text_patch`, add:

```rust
if let Some(v) = &p.ocr_engine {
    t.ocr_engine = v.clone();
}
if let Some(v) = p.ocr_confidence {
    t.ocr_confidence = v;
}
if let Some(v) = p.ocr_uncertain {
    t.ocr_uncertain = v;
}
```

---

## Task 5 — Add OCR quality checker

Create:

```text
crates/koharu-app/src/pipeline/ocr_quality.rs
```

Add:

```rust
#[derive(Debug, Clone, Copy)]
pub struct OcrQualityInput<'a> {
    pub text: Option<&'a str>,
    pub detector_confidence: f32,
    pub ocr_confidence: Option<f32>,
    pub bbox_width: f32,
    pub bbox_height: f32,
    pub is_vertical: bool,
}

#[derive(Debug, Clone)]
pub struct OcrQualityReport {
    pub score: f32,
    pub uncertain: bool,
    pub reasons: Vec<OcrQualityReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrQualityReason {
    EmptyText,
    LowDetectorConfidence,
    LowOcrConfidence,
    BadCharacters,
    TooShortForLargeBox,
    LowJapaneseRatio,
    WeirdRepetition,
}
```

Add:

```rust
pub fn assess_ocr_quality(input: OcrQualityInput<'_>) -> OcrQualityReport
```

Suggested thresholds:

```rust
const LOW_DETECTOR_CONFIDENCE: f32 = 0.45;
const LOW_OCR_CONFIDENCE: f32 = 0.65;
const LARGE_BOX_AREA: f32 = 20_000.0;
const MIN_JP_RATIO_FOR_LONG_TEXT: f32 = 0.35;
```

Suspicious if:

- text empty
- contains `□` or `�`
- detector confidence `< 0.45`
- OCR confidence exists and `< 0.65`
- bbox area is large but text length `<= 2`
- text length `>= 4` and Japanese ratio `< 0.35`
- weird repetition

Do not mark common short manga texts as suspicious:

```text
え？
うん
はい
いや
あ
ん？
…
！？
```

---

## Task 6 — Export ocr_quality module

Find:

```text
crates/koharu-app/src/pipeline/mod.rs
```

Add:

```rust
pub mod ocr_quality;
```

---

## Task 7 — Update existing OCR engines

Files:

```text
crates/koharu-app/src/pipeline/engines/manga_ocr.rs
crates/koharu-app/src/pipeline/engines/mit48px_ocr.rs
crates/koharu-app/src/pipeline/engines/paddle_ocr.rs
```

When creating `TextDataPatch`, change from:

```rust
TextDataPatch {
    text: Some(Some(text)),
    ..Default::default()
}
```

to:

```rust
let report = assess_ocr_quality(OcrQualityInput {
    text: Some(&text),
    detector_confidence: text_data.confidence,
    ocr_confidence: None,
    bbox_width: transform.width,
    bbox_height: transform.height,
    is_vertical: matches!(
        text_data.source_direction,
        Some(koharu_core::TextDirection::Vertical)
    ),
});

TextDataPatch {
    text: Some(Some(text.clone())),
    ocr_engine: Some(Some("manga-ocr".to_string())),
    ocr_confidence: Some(None),
    ocr_uncertain: Some(report.uncertain),
    ..Default::default()
}
```

Use correct engine names:

```text
manga-ocr
mit48px-ocr
paddle-ocr-vl-1.6
```

Do not overwrite detector `confidence`.

---

## Tests

### Core tests

Add tests for:

- apply OCR metadata patch
- undo restores OCR metadata
- old JSON without metadata loads correctly

### OCR quality tests

Add tests for:

- empty text => uncertain
- good Japanese text => not uncertain
- bad chars => uncertain
- low detector confidence => uncertain
- low OCR confidence => uncertain
- large bbox + very short text => uncertain
- common short manga text => not uncertain

---

## Commands

```powershell
cd D:\project\koharu
cargo test -p koharu-core
cargo test -p koharu-app
bun run generate:openapi
cd ui
bun run generate:api
bun run build
```

---

## Acceptance Criteria

- All 3 original OCR engines still run.
- OCR output text nodes now include:
  - `ocrEngine`
  - `ocrConfidence`
  - `ocrUncertain`
- Old project files still load.
- No Unlimited-OCR service required.
- No behavior change except metadata and uncertainty flag.
