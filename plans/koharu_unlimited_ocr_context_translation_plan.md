# Koharu Unlimited-OCR + Context-Aware Translation Implementation Plan

> Mục tiêu: tích hợp `baidu/Unlimited-OCR` vào Koharu theo 3 chế độ:
>
> 1. **Off**: không dùng Unlimited-OCR, giữ nguyên 3 OCR gốc.
> 2. **Smart Fallback**: OCR gốc chạy trước, chỉ case khó mới gọi Unlimited-OCR.
> 3. **Full Unlimited-OCR**: dùng Unlimited-OCR cho toàn bộ OCR.
>
> Đồng thời nâng cấp bước dịch LLM để dùng **rich context JSON** thay vì chỉ truyền list text rời rạc.
>
> Yêu cầu quan trọng: không phá 3 OCR engine gốc:
>
> - `Manga OCR`
> - `MIT 48px OCR`
> - `PaddleOCR-VL`

---

## 0. Coding Agent Rules

### 0.1. General rules

- Không rewrite architecture Koharu.
- Không xoá hoặc rename 3 OCR engine gốc.
- Không làm breaking change với project file cũ.
- Mọi field mới trong scene model phải có `#[serde(default)]`.
- Mọi thay đổi Rust data model phải update `TextDataPatch`, `capture_prev_text`, `apply_text_patch`.
- Mọi thay đổi OpenAPI/schema phải regenerate client UI.
- Unlimited-OCR phải chạy ngoài Rust bằng Python service.
- Rust chỉ đóng vai trò adapter gọi HTTP service.
- Nếu Unlimited-OCR service không chạy, mode `Off` vẫn phải hoạt động bình thường.
- Nếu mode `SmartFallback`, lỗi Unlimited-OCR không được crash toàn pipeline nếu base OCR đã có text đủ dùng; phải mark warning hoặc fallback về OCR gốc.
- Nếu mode `FullUnlimited`, lỗi Unlimited-OCR có thể fail pipeline với error rõ ràng.

### 0.2. Branch

Create branch:

```bash
git checkout -b feat/unlimited-ocr-context-translation
```

### 0.3. Main deliverables

- Rust core data model supports OCR metadata.
- OCR quality checker.
- Unlimited-OCR mode config.
- Python Unlimited-OCR HTTP service.
- Rust Unlimited-OCR client/engine.
- Smart fallback OCR strategy.
- UI setting for Unlimited-OCR mode.
- Rich translation context data model.
- LLM translation engine using rich context.
- Tests.
- Benchmark script/doc.

---

## 1. Current Architecture Summary

### 1.1. Existing OCR engines

Current Koharu has OCR engines under:

```text
crates/koharu-app/src/pipeline/engines/
```

Expected files:

```text
manga_ocr.rs
mit48px_ocr.rs
paddle_ocr.rs
```

They are registered through `inventory::submit! { EngineInfo { ... } }`.

Existing OCR engines should remain valid providers for:

```rust
needs: &[Artifact::TextBoxes],
produces: &[Artifact::OcrText],
```

### 1.2. Artifact flow

Current relevant artifact flow:

```text
SourceImage
→ TextBoxes
→ OcrText
→ Translations
→ RenderedSprites
→ FinalRender
```

`Unlimited-OCR` only belongs in:

```text
TextBoxes → OcrText
```

Context-aware translation belongs in:

```text
OcrText → Translations
```

### 1.3. Current limitation

Current LLM translate engine collects only source text strings:

```text
Vec<String>
```

It does not pass:

- bbox
- reading order
- source direction
- OCR engine
- OCR uncertainty
- visual hint
- speaker hint
- tone hint
- SFX/narration/dialogue role

This plan upgrades that.

---

## 2. Target Behavior

### 2.1. UI behavior

Add a second OCR-related setting, separate from existing OCR engine selection.

Current UI has base OCR engine dropdown:

```text
OCR:
- Manga OCR
- MIT 48px OCR
- PaddleOCR-VL
```

Add:

```text
Unlimited-OCR Mode:
- Off
- Smart fallback
- Full Unlimited-OCR
```

Recommended default after implementation:

```text
Base OCR Engine: Manga OCR
Unlimited-OCR Mode: Smart fallback
Translation: Context-aware LLM enabled
```

But for first safe release, acceptable default is:

```text
Unlimited-OCR Mode: Off
Context-aware LLM: On
```

### 2.2. Runtime behavior

#### Mode A: Off

```text
Detect text boxes
→ selected base OCR engine
→ write OcrText
→ context-aware LLM translation
→ inpaint/render
```

No Python service needed.

#### Mode B: SmartFallback

```text
Detect text boxes
→ selected base OCR engine
→ calculate OCR quality
→ only suspicious boxes go to Unlimited-OCR
→ update suspicious boxes with Unlimited-OCR result
→ context-aware LLM translation
→ inpaint/render
```

Python service needed only when suspicious boxes exist.

#### Mode C: FullUnlimited

```text
Detect text boxes
→ send all boxes/page to Unlimited-OCR
→ write OcrText
→ context-aware LLM translation
→ inpaint/render
```

Python service required.

---

## 3. Add OCR Metadata to Core Model

### 3.1. File

```text
crates/koharu-core/src/scene.rs
```

### 3.2. Add fields to `TextData`

Find:

```rust
pub struct TextData {
    #[serde(default)]
    pub confidence: f32,
    ...
    #[serde(default)]
    pub detector: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub translation: Option<String>,
    ...
}
```

Add after `detector`:

```rust
    /// OCR engine that produced the current `text`.
    /// Examples: "manga-ocr", "mit48px-ocr", "paddle-ocr-vl-1.6", "unlimited-ocr".
    #[serde(default)]
    pub ocr_engine: Option<String>,

    /// OCR confidence if the OCR engine provides a real score.
    /// For engines that do not provide confidence, keep None.
    #[serde(default)]
    pub ocr_confidence: Option<f32>,

    /// Whether current OCR result is suspicious and should be reviewed/fallback.
    #[serde(default)]
    pub ocr_uncertain: bool,

    /// Extra context useful for translation.
    /// May be produced by Unlimited-OCR context mode or by a VLM/context analyzer.
    #[serde(default)]
    pub translation_context: Option<TextTranslationContext>,
```

### 3.3. Add new struct

In same file, near `TextData`:

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextTranslationContext {
    /// dialogue / narration / sfx / sign / unknown
    #[serde(default)]
    pub role: Option<String>,

    /// Soft hint only. Do not treat as guaranteed.
    #[serde(default)]
    pub speaker_hint: Option<String>,

    /// Soft hint only.
    #[serde(default)]
    pub emotion_hint: Option<String>,

    /// Visual context around the text box.
    #[serde(default)]
    pub visual_hint: Option<String>,

    /// Note for translation model.
    #[serde(default)]
    pub translation_note: Option<String>,

    /// True when context was inferred with low certainty.
    #[serde(default)]
    pub context_uncertain: bool,
}
```

### 3.4. Backward compatibility

Because all fields use `#[serde(default)]`, old project files without these fields must load normally.

---

## 4. Add OCR Metadata to Patches

### 4.1. File

```text
crates/koharu-core/src/op.rs
```

### 4.2. Imports

Ensure `TextTranslationContext` is imported if needed:

```rust
use crate::scene::{
    ImageData, ImageRole, MaskData, MaskRole, Node, NodeId, NodeKind, NodeKindTag, Page, PageId,
    ProjectStyle, Scene, TextData, TextTranslationContext, Transform,
};
```

### 4.3. Extend `TextDataPatch`

Add after `detector`:

```rust
    #[serde(default)]
    pub ocr_engine: Option<Option<String>>,

    #[serde(default)]
    pub ocr_confidence: Option<Option<f32>>,

    #[serde(default)]
    pub ocr_uncertain: Option<bool>,

    #[serde(default)]
    pub translation_context: Option<Option<TextTranslationContext>>,
```

Expected semantics:

```text
None                 = do not patch this field
Some(Some(value))    = set value
Some(None)           = clear optional value
```

For `ocr_uncertain`, use `Option<bool>`.

### 4.4. Update `capture_prev_text`

Add:

```rust
ocr_engine: p.ocr_engine.as_ref().map(|_| data.ocr_engine.clone()),
ocr_confidence: p.ocr_confidence.as_ref().map(|_| data.ocr_confidence),
ocr_uncertain: p.ocr_uncertain.as_ref().map(|_| data.ocr_uncertain),
translation_context: p
    .translation_context
    .as_ref()
    .map(|_| data.translation_context.clone()),
```

Place near `detector`, `text`, `translation`.

### 4.5. Update `apply_text_patch`

Add:

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
if let Some(v) = &p.translation_context {
    t.translation_context = v.clone();
}
```

### 4.6. Tests

Add or update tests in `crates/koharu-core/src/op.rs`:

- Updating `ocr_engine` should be applied.
- Updating `ocr_confidence` should be applied.
- Updating `ocr_uncertain` should be applied.
- Updating `translation_context` should be applied.
- Undo should restore previous values.

---

## 5. Add OCR Quality Checker

### 5.1. New file

Create:

```text
crates/koharu-app/src/pipeline/ocr_quality.rs
```

### 5.2. Public API

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

### 5.3. Function

```rust
pub fn assess_ocr_quality(input: OcrQualityInput<'_>) -> OcrQualityReport
```

### 5.4. Heuristic rules

Use conservative rules.

Suggested thresholds:

```rust
const LOW_DETECTOR_CONFIDENCE: f32 = 0.45;
const LOW_OCR_CONFIDENCE: f32 = 0.65;
const LARGE_BOX_AREA: f32 = 20_000.0;
const MIN_JP_RATIO_FOR_LONG_TEXT: f32 = 0.35;
```

Rules:

- Empty text => uncertain.
- Contains bad chars: `□`, `�` => uncertain.
- Detector confidence < 0.45 => uncertain.
- OCR confidence exists and < 0.65 => uncertain.
- Large bbox area and text length <= 2 => uncertain.
- Text length >= 4 and Japanese char ratio < 0.35 => uncertain.
- Same char repeated too many times, unless it is valid manga elongation like `ーーー`, should be suspicious with low weight.
- Do not mark common short texts as uncertain just because short:
  - `え？`
  - `うん`
  - `はい`
  - `いや`
  - `あ`
  - `ん？`
  - `…`
  - `！？`

### 5.5. Helper functions

```rust
fn japanese_ratio(text: &str) -> f32
fn contains_bad_chars(text: &str) -> bool
fn is_allowed_short_manga_text(text: &str) -> bool
fn has_weird_repetition(text: &str) -> bool
```

### 5.6. Unit tests

Create tests for:

- empty text
- good Japanese text
- bad chars
- large bbox with 1 char
- mostly Latin noise
- common short manga text should not be marked uncertain
- low detector confidence
- low OCR confidence

---

## 6. Add Unlimited-OCR Mode Config

### 6.1. Decide config location

Search existing pipeline config/options files:

```text
crates/koharu-app/src/pipeline/
crates/koharu-rpc/src/routes/pipelines.rs
ui/lib/api/schemas/pipelineConfig.ts
```

Current `PipelineRunOptions` exists in:

```text
crates/koharu-app/src/pipeline/engine.rs
```

It currently includes:

```rust
pub struct PipelineRunOptions {
    pub target_language: Option<String>,
    pub system_prompt: Option<String>,
    pub default_font: Option<String>,
    pub text_node_ids: Option<Vec<NodeId>>,
    pub region: Option<Region>,
    pub reading_order: Option<ReadingOrder>,
}
```

Add:

```rust
pub unlimited_ocr_mode: UnlimitedOcrMode,
pub unlimited_ocr_url: Option<String>,
```

### 6.2. Add enum

Create in an appropriate shared config module, or near pipeline options if simpler:

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum UnlimitedOcrMode {
    #[default]
    Off,
    SmartFallback,
    Full,
}
```

If `ToSchema` or `JsonSchema` imports are not available in that file, place enum in existing config schema module where those derive macros are already used.

### 6.3. Wire through RPC

Find pipeline start request type. It likely lives in:

```text
crates/koharu-rpc/src/routes/pipelines.rs
```

Add fields:

```rust
#[serde(default)]
pub unlimited_ocr_mode: UnlimitedOcrMode,

#[serde(default)]
pub unlimited_ocr_url: Option<String>,
```

Map them into `PipelineRunOptions`.

### 6.4. UI schema/client

After Rust changes:

```bash
bun run generate:openapi
cd ui
bun run generate:api
```

---

## 7. Python Unlimited-OCR Service

### 7.1. Create folder

At repo root:

```text
services/unlimited-ocr/
```

Files:

```text
services/unlimited-ocr/README.md
services/unlimited-ocr/requirements.txt
services/unlimited-ocr/server.py
services/unlimited-ocr/schemas.py
services/unlimited-ocr/run.ps1
services/unlimited-ocr/run.sh
```

### 7.2. requirements.txt

Start with:

```text
fastapi
uvicorn[standard]
pydantic
pillow
torch
torchvision
transformers
einops
addict
easydict
python-multipart
```

Optional:

```text
orjson
```

### 7.3. API design

#### Health endpoint

```http
GET /health
```

Response:

```json
{
  "ok": true,
  "model_loaded": true,
  "device": "cuda",
  "model": "baidu/Unlimited-OCR"
}
```

#### Batch crop OCR

```http
POST /ocr/crops
```

Request:

```json
{
  "images": [
    {
      "id": "node-id-or-index",
      "image_base64": "..."
    }
  ],
  "language_hint": "ja",
  "return_context": false
}
```

Response:

```json
{
  "items": [
    {
      "id": "node-id-or-index",
      "text": "何してるの？",
      "confidence": null,
      "uncertain": false,
      "role": null,
      "speaker_hint": null,
      "emotion_hint": null,
      "visual_hint": null,
      "translation_note": null
    }
  ],
  "page_context": null,
  "warnings": []
}
```

#### Page/contact-sheet OCR

```http
POST /ocr/page
```

Request:

```json
{
  "page_image_base64": "...",
  "boxes": [
    {
      "id": "node-id",
      "bbox": [x, y, w, h],
      "order": 1
    }
  ],
  "language_hint": "ja",
  "return_context": true
}
```

Response same shape.

### 7.4. Important service behavior

- Load model once at startup.
- Do not reload model per request.
- Use `torch_dtype=torch.bfloat16` on CUDA if supported.
- Accept `UNLIMITED_OCR_MODEL` env var, default `baidu/Unlimited-OCR`.
- Accept `UNLIMITED_OCR_DEVICE` env var, default `cuda`.
- If CUDA not available, return clear error at startup unless `UNLIMITED_OCR_DEVICE=cpu`.
- Log processing time per request.
- Limit max images per request using env var:
  - `MAX_CROPS_PER_REQUEST=64`
- Limit max image size using env var:
  - `MAX_IMAGE_SIDE=2048`

### 7.5. Strict JSON parsing

Model outputs may not be strict JSON. Service must:

- Try parse JSON.
- If parse fails, return raw text in `text`.
- Add warning:
  - `"model_output_not_json"`
- Never crash on malformed model output if text can be recovered.

### 7.6. README

Include:

```powershell
cd services\unlimited-ocr
python -m venv .venv
.\.venv\Scripts\activate
pip install -r requirements.txt
uvicorn server:app --host 127.0.0.1 --port 7862
```

---

## 8. Rust Unlimited-OCR HTTP Client

### 8.1. New file

```text
crates/koharu-app/src/pipeline/unlimited_ocr_client.rs
```

### 8.2. Cargo dependency

In:

```text
crates/koharu-app/Cargo.toml
```

Ensure:

```toml
reqwest = { workspace = true }
```

### 8.3. Types

```rust
#[derive(Debug, Clone)]
pub struct UnlimitedOcrClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Serialize)]
pub struct UnlimitedCropImage {
    pub id: String,
    pub image_base64: String,
}

#[derive(Debug, Serialize)]
pub struct UnlimitedCropRequest {
    pub images: Vec<UnlimitedCropImage>,
    pub language_hint: Option<String>,
    pub return_context: bool,
}

#[derive(Debug, Deserialize)]
pub struct UnlimitedOcrResponse {
    pub items: Vec<UnlimitedOcrItem>,
    pub page_context: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UnlimitedOcrItem {
    pub id: String,
    pub text: String,
    pub confidence: Option<f32>,
    pub uncertain: bool,
    pub role: Option<String>,
    pub speaker_hint: Option<String>,
    pub emotion_hint: Option<String>,
    pub visual_hint: Option<String>,
    pub translation_note: Option<String>,
}
```

### 8.4. Methods

```rust
impl UnlimitedOcrClient {
    pub fn new(base_url: impl Into<String>) -> Self;
    pub async fn health(&self) -> Result<()>;
    pub async fn ocr_crops(&self, req: UnlimitedCropRequest) -> Result<UnlimitedOcrResponse>;
}
```

### 8.5. Error handling

- Timeout default: 180 seconds.
- If service unreachable:
  - In SmartFallback: return warning and keep base OCR.
  - In Full: return error.

Use `anyhow::Context` for clear messages:

```text
failed to connect to Unlimited-OCR service at http://127.0.0.1:7862
```

---

## 9. Rust Unlimited-OCR Engine

### 9.1. New file

```text
crates/koharu-app/src/pipeline/engines/unlimited_ocr.rs
```

### 9.2. Register module

In:

```text
crates/koharu-app/src/pipeline/engines/mod.rs
```

Add:

```rust
pub mod unlimited_ocr;
```

### 9.3. Behavior

This engine implements Full Unlimited-OCR only.

It should:

1. Collect text nodes.
2. Load source image.
3. Crop each text box.
4. Batch-send crops to Python service.
5. Update each text node:
   - `text`
   - `ocr_engine = "unlimited-ocr"`
   - `ocr_confidence`
   - `ocr_uncertain`
   - `translation_context`

### 9.4. EngineInfo

```rust
inventory::submit! {
    EngineInfo {
        id: "unlimited-ocr",
        name: "Unlimited OCR",
        needs: &[Artifact::TextBoxes],
        produces: &[Artifact::OcrText],
        load: |_runtime, _cpu| Box::pin(async move {
            let url = std::env::var("UNLIMITED_OCR_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:7862".to_string());
            Ok(Box::new(Model::new(url)) as Box<dyn Engine>)
        }),
    }
}
```

### 9.5. Image encoding helper

Add helper to encode crop as PNG base64.

```rust
fn image_to_base64_png(image: &DynamicImage) -> Result<String>
```

### 9.6. Mapping safety

Use text node id as OCR item id:

```rust
node_id.to_string()
```

Build a `HashMap<String, UnlimitedOcrItem>` from response.

If a node result is missing:

- Set `ocr_uncertain = true`.
- Keep existing text if any.
- Add warning if pipeline supports warnings.
- If no warning mechanism, log with `tracing::warn!`.

---

## 10. Smart Fallback OCR Strategy

### 10.1. Best architecture

Do not rewrite the 3 base OCR engines.

Add new strategy engine:

```text
crates/koharu-app/src/pipeline/engines/smart_ocr.rs
```

Register:

```rust
id: "smart-ocr"
name: "Smart OCR"
needs: &[Artifact::TextBoxes]
produces: &[Artifact::OcrText]
```

But Smart OCR needs to know selected base OCR engine. If that is difficult with current pipeline config, implement SmartFallback in pipeline orchestration instead of a separate engine.

### 10.2. Simpler implementation option

Use two-step pipeline:

```text
Base OCR engine runs first.
Then smart fallback engine runs and only updates uncertain nodes.
```

Create engine:

```text
unlimited-ocr-fallback
```

```rust
id: "unlimited-ocr-fallback"
name: "Unlimited OCR Fallback"
needs: &[Artifact::OcrText]
produces: &[Artifact::OcrText]
```

Important: producing same artifact as it needs may confuse DAG if selected together. If pipeline resolver does not support same artifact refinement, do not use this approach.

### 10.3. Safer implementation option

Implement SmartFallback as orchestration-level logic:

- If mode Off:
  - run selected base OCR engine.
- If mode SmartFallback:
  - run selected base OCR engine.
  - then call fallback routine on uncertain nodes.
- If mode Full:
  - run `unlimited-ocr` engine.

### 10.4. Fallback routine

Create:

```text
crates/koharu-app/src/pipeline/unlimited_ocr_fallback.rs
```

Function:

```rust
pub async fn apply_unlimited_ocr_fallback(
    ctx: EngineCtx<'_>,
    service_url: &str,
) -> Result<Vec<Op>>
```

This function:

1. Inspect text nodes after base OCR.
2. Use `assess_ocr_quality`.
3. Collect only suspicious nodes.
4. Send suspicious crops to service.
5. Return `UpdateNode` ops.

### 10.5. SmartFallback failure rule

If Unlimited-OCR service fails:

- Log warning.
- Keep base OCR text.
- Set `ocr_uncertain = true`.
- Do not fail entire pipeline unless no base OCR text exists.

---

## 11. Update Existing OCR Engines to Write Metadata

### 11.1. Files

```text
crates/koharu-app/src/pipeline/engines/manga_ocr.rs
crates/koharu-app/src/pipeline/engines/mit48px_ocr.rs
crates/koharu-app/src/pipeline/engines/paddle_ocr.rs
```

### 11.2. Required patch fields

When setting OCR result, update:

```rust
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

### 11.3. Compute quality report

Use:

```rust
let report = assess_ocr_quality(OcrQualityInput {
    text: Some(&text),
    detector_confidence: existing_text_data.confidence,
    ocr_confidence: None,
    bbox_width: transform.width,
    bbox_height: transform.height,
    is_vertical: existing_text_data.source_direction == Some(TextDirection::Vertical),
});
```

### 11.4. Do not overwrite detector confidence

Do not set `confidence` from OCR engines unless detector confidence is intentionally changed.

---

## 12. Context-Aware Translation Data Model

### 12.1. New file

```text
crates/koharu-app/src/pipeline/translation_context.rs
```

### 12.2. Types

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationBlock {
    pub id: String,
    pub text: String,
    pub order: usize,
    pub bbox: [f32; 4],
    pub position: String,
    pub source_direction: Option<String>,
    pub detector_confidence: f32,
    pub detector: Option<String>,
    pub ocr_engine: Option<String>,
    pub ocr_confidence: Option<f32>,
    pub ocr_uncertain: bool,
    pub role: Option<String>,
    pub speaker_hint: Option<String>,
    pub emotion_hint: Option<String>,
    pub visual_hint: Option<String>,
    pub translation_note: Option<String>,
    pub previous_text: Option<String>,
    pub next_text: Option<String>,
}
```

### 12.3. Builder

```rust
pub fn build_translation_blocks(
    scene: &Scene,
    page: PageId,
    allowed_ids: Option<&[NodeId]>,
    reading_order: Option<ReadingOrder>,
) -> Vec<(NodeId, TranslationBlock)>
```

### 12.4. Reading order

Use existing `text_nodes(scene, page)` then sort by reading order.

If existing sorting helper is available:

```rust
sort_manga_reading_order
```

Use it carefully.

If not easy, use current order from `text_nodes` for v1 and mark TODO.

### 12.5. Position helper

Given page width/height and bbox center, produce:

```text
top-left
top-center
top-right
middle-left
middle-center
middle-right
bottom-left
bottom-center
bottom-right
```

### 12.6. Previous/next context

After sorting, fill:

```rust
previous_text
next_text
```

Use neighboring OCR text.

---

## 13. Update LLM Translation Engine

### 13.1. File

```text
crates/koharu-app/src/pipeline/engines/llm_translate.rs
```

### 13.2. Current behavior

Currently:

```rust
let sources: Vec<String> = targets.iter().map(|(_, s)| s.clone()).collect();
ctx.llm.translate_texts(&sources, ...)
```

### 13.3. New behavior

Replace collection with rich blocks:

```rust
let targets = build_translation_blocks(
    ctx.scene,
    ctx.page,
    ctx.options.text_node_ids.as_deref(),
    ctx.options.reading_order,
);
```

Call new LLM method:

```rust
ctx.llm
    .translate_text_blocks(&blocks, target_language, system_prompt)
    .await?;
```

If adding new LLM method is too invasive, serialize rich context into strings and call existing method as fallback.

### 13.4. Preferred new LLM input prompt

System prompt:

```text
You are a professional Japanese-to-Vietnamese manga translator.
Translate each text box naturally into Vietnamese.
Use reading order, previous/next text, position, role, speaker hints, emotion hints, and visual hints as soft context.
If hints conflict with the Japanese text, prioritize the Japanese text.
Preserve tone, brevity, and manga style.
For SFX, translate or adapt naturally for Vietnamese manga if appropriate.
Return strict JSON only.
```

User payload:

```json
{
  "targetLanguage": "Vietnamese",
  "readingOrder": "rtl",
  "blocks": [
    {
      "id": "node-id",
      "text": "何してるの？",
      "order": 1,
      "position": "top-right",
      "sourceDirection": "vertical",
      "ocrEngine": "manga-ocr",
      "ocrUncertain": false,
      "role": "dialogue",
      "speakerHint": "girl",
      "emotionHint": "annoyed",
      "visualHint": "girl is confronting boy",
      "previousText": null,
      "nextText": "別に……"
    }
  ]
}
```

Expected output:

```json
[
  {
    "id": "node-id",
    "translation": "Cậu đang làm gì vậy?"
  }
]
```

### 13.5. Output mapping

Map by `id`, not by list index only.

If output is missing an id:

- fallback to index mapping if length matches
- otherwise skip and log warning

### 13.6. Preserve existing behavior fallback

If rich translation fails due to JSON parse error, fallback to existing `translate_texts` to avoid blocking users.

---

## 14. UI Changes

### 14.1. Locate pipeline settings UI

Search:

```text
ui/components
ui/lib/api/schemas/pipelineConfig.ts
ui/lib/api/schemas/startPipelineRequest.ts
```

Find current OCR engine dropdown.

### 14.2. Add mode dropdown

Label:

```text
Unlimited-OCR Mode
```

Options:

```text
Off
Smart fallback
Full Unlimited-OCR
```

Values:

```text
off
smart-fallback
full
```

### 14.3. Add service URL input

Only show when mode is not Off:

```text
Unlimited-OCR Service URL
default: http://127.0.0.1:7862
```

### 14.4. Add status check button

Button:

```text
Check Unlimited-OCR Service
```

Calls backend or direct frontend health check.

Display:

```text
Connected / Not connected
```

### 14.5. UX warnings

If mode `Smart fallback` or `Full` and service not reachable:

- show warning
- allow save
- pipeline will error or fallback depending on mode

### 14.6. Display OCR metadata

Optional but useful:

In text block panel, show small info:

```text
OCR: manga-ocr
Uncertain: yes/no
```

For `ocr_uncertain = true`, show warning icon.

---

## 15. OpenAPI and Client Generation

After Rust schema changes:

```bash
cd <repo-root>
bun run generate:openapi
cd ui
bun run generate:api
```

Then run:

```bash
cd ui
bun run build
```

Fix generated TS type errors.

---

## 16. Tests

### 16.1. Rust unit tests

Add tests for:

- `TextData` old JSON loads without new fields.
- `TextDataPatch` applies OCR metadata.
- Undo restores OCR metadata.
- `assess_ocr_quality` rules.
- Translation block builder:
  - correct bbox
  - correct position
  - skips blank OCR text
  - includes metadata
  - preserves allowed `text_node_ids`

### 16.2. Rust integration tests

Add or update pipeline tests:

- Mode Off does not call Unlimited-OCR.
- Mode Full uses Unlimited-OCR engine.
- Mode SmartFallback:
  - good OCR text does not call service
  - suspicious OCR text calls service
  - service failure keeps base OCR text and marks uncertain

For HTTP service tests, use a mock HTTP server, not real Unlimited-OCR.

### 16.3. Python tests

Under:

```text
services/unlimited-ocr/tests/
```

Test:

- `/health`
- `/ocr/crops` request schema
- malformed model output handling
- empty image list
- max crops exceeded

Mock model inference; do not load real model in CI.

### 16.4. UI tests

At minimum:

```bash
cd ui
bun run lint
bun run build
```

If test infra exists:

- settings dropdown renders
- request payload includes `unlimitedOcrMode`
- service URL input appears only when mode is not Off

---

## 17. Benchmark Plan

### 17.1. Create benchmark doc

```text
docs/benchmarks/unlimited-ocr.md
```

### 17.2. Metrics

Track:

```text
pages
text boxes
base OCR time
Unlimited-OCR time
fallback count
total pipeline time
OCR uncertain count before fallback
OCR uncertain count after fallback
manual correction count
translation quality notes
```

### 17.3. Modes to benchmark

```text
Off
Smart fallback
Full Unlimited-OCR crop batch
Full Unlimited-OCR page/contact-sheet
```

### 17.4. Sample data

Use legal manga samples only.

Do not include copyrighted manga samples in repo.

Allowed:

- User-provided local samples not committed.
- Public-domain or permissively licensed manga images.
- Research dataset samples only if license permits local testing.

---

## 18. Implementation Order

### Phase 1: Safe metadata foundation

1. Add `TextTranslationContext`.
2. Add OCR metadata fields to `TextData`.
3. Add fields to `TextDataPatch`.
4. Update patch apply/undo.
5. Add tests.
6. Regenerate OpenAPI/client.

DoD:

```text
cargo test -p koharu-core
ui build passes
old project JSON loads
```

### Phase 2: OCR quality checker

1. Add `ocr_quality.rs`.
2. Add unit tests.
3. Update existing 3 OCR engines to write:
   - `ocr_engine`
   - `ocr_confidence`
   - `ocr_uncertain`

DoD:

```text
3 OCR engines still run
metadata appears in scene JSON/API
```

### Phase 3: Context-aware translation without Unlimited-OCR

1. Add `translation_context.rs`.
2. Update `llm_translate.rs`.
3. Add fallback to old translation method.
4. Add tests.

DoD:

```text
Translation uses rich context JSON
If rich JSON parse fails, old translation still works
```

### Phase 4: Python Unlimited-OCR service

1. Create `services/unlimited-ocr`.
2. Implement API.
3. Implement mocked tests.
4. Document setup.

DoD:

```text
GET /health works
POST /ocr/crops works with mock or real model
service does not reload model per request
```

### Phase 5: Rust Unlimited-OCR integration

1. Add HTTP client.
2. Add `unlimited_ocr.rs` engine.
3. Add service URL config.
4. Add Full mode.

DoD:

```text
Mode Full updates text nodes with ocrEngine=unlimited-ocr
clear error if service unavailable
```

### Phase 6: SmartFallback mode

1. Add mode config.
2. Implement fallback orchestration.
3. Only suspicious boxes go to Unlimited-OCR.
4. Service failure does not destroy base OCR result.

DoD:

```text
Good OCR does not call service
Bad OCR calls service
Fallback updates only selected nodes
```

### Phase 7: UI and polish

1. Add Unlimited-OCR Mode dropdown.
2. Add service URL input.
3. Add status check.
4. Display OCR metadata in text panel.

DoD:

```text
UI can choose all 3 modes
Pipeline request contains correct settings
```

### Phase 8: Benchmark and defaults

1. Add benchmark doc.
2. Run sample tests.
3. Decide default mode.
4. Update docs.

DoD:

```text
Performance numbers documented
Default mode chosen
```

---

## 19. Acceptance Criteria

### 19.1. Functional

- 3 original OCR engines still work.
- Off mode behaves like original Koharu except metadata is added.
- SmartFallback calls Unlimited-OCR only for suspicious OCR results.
- Full mode calls Unlimited-OCR for all text nodes.
- Translation LLM receives rich context.
- Project files remain backward-compatible.
- UI exposes mode and URL.

### 19.2. Reliability

- Service unavailable:
  - Off: unaffected.
  - SmartFallback: warns/keeps base OCR.
  - Full: clear error.
- Malformed Unlimited-OCR output:
  - no panic
  - mark uncertain
  - log warning
- Missing OCR item id:
  - no panic
  - mark uncertain
- JSON translation parse failure:
  - fallback to old translation path.

### 19.3. Performance

- Unlimited-OCR model loads once.
- Crop OCR uses batch requests.
- SmartFallback sends only suspicious boxes.
- No per-bubble HTTP request loop unless batch size is 1 by config.

### 19.4. Code quality

- No duplicated large OCR code.
- Python service isolated under `services/unlimited-ocr`.
- Rust Unlimited-OCR integration isolated in client + engine/fallback module.
- Tests cover new metadata, fallback, and context translation.

---

## 20. Suggested Commit Plan

### Commit 1

```text
feat(core): add OCR metadata and translation context fields
```

### Commit 2

```text
feat(pipeline): add OCR quality checker
```

### Commit 3

```text
feat(ocr): annotate existing OCR engines with metadata
```

### Commit 4

```text
feat(translation): build rich context for LLM translation
```

### Commit 5

```text
feat(service): add Unlimited-OCR Python service
```

### Commit 6

```text
feat(ocr): add Unlimited-OCR HTTP client and engine
```

### Commit 7

```text
feat(pipeline): add Unlimited-OCR modes and smart fallback
```

### Commit 8

```text
feat(ui): expose Unlimited-OCR settings
```

### Commit 9

```text
test: add OCR fallback and context translation coverage
```

### Commit 10

```text
docs: add Unlimited-OCR setup and benchmark guide
```

---

## 21. Non-Goals for First Version

Do not implement these in v1:

- Port Unlimited-OCR to Rust.
- Replace all Koharu OCR engines.
- Automatic character identity tracking across pages.
- Perfect speaker detection.
- Full manga chapter memory/context across many pages.
- Fine-tuning OCR/translation models.
- Committing copyrighted manga samples.

---

## 22. Risk Notes

### Risk: Unlimited-OCR is slow

Mitigation:

- SmartFallback default.
- Batch crop requests.
- Full mode only for high quality.

### Risk: VLM metadata hallucination

Mitigation:

- Store as hints.
- Prompt translation LLM to prioritize Japanese OCR text.
- Mark `context_uncertain`.

### Risk: SmartFallback DAG complexity

Mitigation:

- Prefer orchestration-level implementation.
- Avoid engine that both needs and produces `OcrText` unless DAG supports it.

### Risk: API schema breakage

Mitigation:

- `#[serde(default)]` for all new fields.
- Regenerate OpenAPI/client.
- Add old JSON load test.

---

## 23. Final Target Pipeline

### Off

```text
SourceImage
→ TextBoxes
→ Base OCR
→ OcrText + OCR metadata
→ Rich Context Builder
→ Context-aware LLM Translation
→ Translations
→ Inpaint
→ Render
```

### SmartFallback

```text
SourceImage
→ TextBoxes
→ Base OCR
→ OcrText + OCR metadata
→ OCR Quality Checker
→ Unlimited-OCR only for uncertain nodes
→ Updated OcrText + richer metadata
→ Rich Context Builder
→ Context-aware LLM Translation
→ Translations
→ Inpaint
→ Render
```

### Full Unlimited-OCR

```text
SourceImage
→ TextBoxes
→ Unlimited-OCR all nodes/page
→ OcrText + rich metadata
→ Rich Context Builder
→ Context-aware LLM Translation
→ Translations
→ Inpaint
→ Render
```
