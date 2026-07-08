# Milestone 3 — Full Unlimited-OCR Mode

## Goal

Add a working Full Unlimited-OCR mode.

In this mode, Koharu sends all detected text boxes to a Python service running `baidu/Unlimited-OCR`, receives OCR text/metadata, and writes results back into text nodes.

---

## Scope

Implement:

1. Python Unlimited-OCR service
2. Rust HTTP client
3. Rust `unlimited-ocr` engine
4. Full Unlimited-OCR mode

Do not implement:

- Smart fallback
- Only-suspicious-box routing
- Complex page/contact-sheet OCR in v1

---

## Architecture

```text
Koharu Rust pipeline
→ crop text boxes
→ POST /ocr/crops
→ Python service runs baidu/Unlimited-OCR
→ JSON response
→ Koharu updates TextData
```

Python runs the model. Rust only calls HTTP and maps results to nodes.

---

## Part A — Python service

### A1. Create service folder

```text
services/unlimited-ocr/
```

Files:

```text
README.md
requirements.txt
server.py
schemas.py
run.ps1
run.sh
```

### A2. requirements.txt

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

### A3. API

#### GET /health

Response:

```json
{
  "ok": true,
  "modelLoaded": true,
  "device": "cuda",
  "model": "baidu/Unlimited-OCR"
}
```

#### POST /ocr/crops

Request:

```json
{
  "images": [
    {
      "id": "node-id",
      "imageBase64": "..."
    }
  ],
  "languageHint": "ja",
  "returnContext": true
}
```

Response:

```json
{
  "items": [
    {
      "id": "node-id",
      "text": "何してるの？",
      "confidence": null,
      "uncertain": false,
      "role": null,
      "speakerHint": null,
      "emotionHint": null,
      "visualHint": null,
      "translationNote": null
    }
  ],
  "pageContext": null,
  "warnings": []
}
```

### A4. Service behavior

- Load model once at startup.
- Do not reload model per request.
- Default model:
  - `baidu/Unlimited-OCR`
- Default URL:
  - `127.0.0.1:7862`
- Use env vars:
  - `UNLIMITED_OCR_MODEL`
  - `UNLIMITED_OCR_DEVICE`
  - `MAX_CROPS_PER_REQUEST`
  - `MAX_IMAGE_SIDE`
- If CUDA unavailable and device is CUDA, return clear startup error.
- If model output is malformed, return best-effort text and warning.
- Never crash on one bad crop; mark that item uncertain.

### A5. run.ps1

```powershell
python -m venv .venv
.\.venv\Scripts\activate
pip install -r requirements.txt
uvicorn server:app --host 127.0.0.1 --port 7862
```

---

## Part B — Rust HTTP client

### B1. Cargo dependency

File:

```text
crates/koharu-app/Cargo.toml
```

Add if missing:

```toml
reqwest = { workspace = true }
```

### B2. Create client file

```text
crates/koharu-app/src/pipeline/unlimited_ocr_client.rs
```

### B3. Types

```rust
#[derive(Debug, Clone)]
pub struct UnlimitedOcrClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlimitedCropImage {
    pub id: String,
    pub image_base64: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlimitedCropRequest {
    pub images: Vec<UnlimitedCropImage>,
    pub language_hint: Option<String>,
    pub return_context: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlimitedOcrResponse {
    pub items: Vec<UnlimitedOcrItem>,
    pub page_context: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
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

### B4. Methods

```rust
impl UnlimitedOcrClient {
    pub fn new(base_url: impl Into<String>) -> Self;

    pub async fn health(&self) -> anyhow::Result<()>;

    pub async fn ocr_crops(
        &self,
        req: UnlimitedCropRequest,
    ) -> anyhow::Result<UnlimitedOcrResponse>;
}
```

Use timeout:

```rust
std::time::Duration::from_secs(180)
```

Error message must include service URL.

---

## Part C — Rust Unlimited-OCR engine

### C1. Create file

```text
crates/koharu-app/src/pipeline/engines/unlimited_ocr.rs
```

### C2. Register module

File:

```text
crates/koharu-app/src/pipeline/engines/mod.rs
```

Add:

```rust
pub mod unlimited_ocr;
```

### C3. Engine behavior

1. Collect text nodes.
2. Load source image.
3. Crop each text box with existing crop helper.
4. Encode crops as PNG base64.
5. Send one batch request to `/ocr/crops`.
6. Map results by node id.
7. Update each node:
   - `text`
   - `ocr_engine = "unlimited-ocr"`
   - `ocr_confidence`
   - `ocr_uncertain`
   - optional `translation_context`

### C4. EngineInfo

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

### C5. Translation context mapping

If response has metadata, map to:

```rust
TextTranslationContext {
    role: item.role,
    speaker_hint: item.speaker_hint,
    emotion_hint: item.emotion_hint,
    visual_hint: item.visual_hint,
    translation_note: item.translation_note,
    context_uncertain: item.uncertain,
}
```

If `TextTranslationContext` does not exist yet, skip context fields and only write OCR metadata.

---

## Part D — Full mode config

### D1. Add enum

Add config enum in appropriate pipeline config file:

```rust
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnlimitedOcrMode {
    #[default]
    Off,
    SmartFallback,
    Full,
}
```

For this milestone, only implement:

```text
Off
Full
```

`SmartFallback` can exist in enum but return `todo` or behave like Off until Milestone 4.

### D2. Add to PipelineRunOptions

```rust
pub unlimited_ocr_mode: UnlimitedOcrMode,
pub unlimited_ocr_url: Option<String>,
```

### D3. Pipeline behavior

If mode is `Full`:

- Use `unlimited-ocr` engine for OCR step.
- Do not run base OCR engine.

If service unavailable:

- fail with clear error.

---

## Tests

### Rust tests

Use mock HTTP server.

Test:

- `/ocr/crops` response maps to correct node id
- missing item id marks node uncertain
- service error returns clear error
- engine writes `ocrEngine = unlimited-ocr`

### Python tests

Do not load real model in CI. Mock model inference.

Test:

- health endpoint
- crop OCR endpoint shape
- empty images
- malformed model output
- max crop limit

---

## Commands

Terminal 1:

```powershell
cd D:\project\koharu\services\unlimited-ocr
python -m venv .venv
.\.venv\Scripts\activate
pip install -r requirements.txt
uvicorn server:app --host 127.0.0.1 --port 7862
```

Terminal 2:

```powershell
cd D:\project\koharu
$env:UNLIMITED_OCR_URL="http://127.0.0.1:7862"
cargo run -p koharu -- --cpu --port 4000 --headless --debug
```

Terminal 3:

```powershell
cd D:\project\koharu\ui
bun dev
```

---

## Acceptance Criteria

- Full Unlimited-OCR mode works end-to-end.
- Python service loads model once.
- Rust sends crop batch, not one request per crop.
- Text nodes are updated with `ocrEngine = unlimited-ocr`.
- Existing OCR engines still work when mode is Off.
- Clear error if service unavailable in Full mode.
