# Milestone 4 — Smart Fallback Unlimited-OCR

## Goal

Implement Smart Fallback mode:

```text
Base OCR first
→ check OCR quality
→ only suspicious text boxes go to Unlimited-OCR
→ update those boxes
→ translation uses final OCR text + metadata
```

This milestone depends on:

- Milestone 2: OCR metadata + quality checker
- Milestone 3: Python service + Rust Unlimited-OCR client

---

## Scope

Implement:

1. SmartFallback mode behavior
2. Only send uncertain/suspicious boxes to Unlimited-OCR
3. Service failure should not destroy base OCR results
4. UI mode option
5. Tests

Do not implement:

- Character identity tracking
- Full chapter memory
- Perfect speaker detection
- Fine-tuning

---

## Target behavior

### Mode Off

```text
Base OCR only
No Unlimited-OCR
```

### Mode Full

```text
Unlimited-OCR for all boxes
Base OCR skipped
```

### Mode SmartFallback

```text
Base OCR
→ ocr_quality
→ suspicious boxes only
→ Unlimited-OCR
→ update suspicious boxes
```

---

## Task 1 — Wire SmartFallback mode

Find pipeline orchestration code that decides which engines run.

Search:

```text
PipelineRunOptions
start pipeline
selected engines
Artifact::OcrText
```

Behavior:

```rust
match options.unlimited_ocr_mode {
    UnlimitedOcrMode::Off => {
        run_selected_base_ocr();
    }
    UnlimitedOcrMode::Full => {
        run_unlimited_ocr();
    }
    UnlimitedOcrMode::SmartFallback => {
        run_selected_base_ocr();
        run_unlimited_ocr_fallback_for_uncertain_nodes();
    }
}
```

Important:

- Do not implement fallback as an engine that both needs and produces `OcrText` unless DAG supports that.
- Prefer orchestration-level implementation.

---

## Task 2 — Create fallback module

Create:

```text
crates/koharu-app/src/pipeline/unlimited_ocr_fallback.rs
```

Add function:

```rust
pub async fn apply_unlimited_ocr_fallback(
    ctx: EngineCtx<'_>,
    service_url: &str,
) -> anyhow::Result<Vec<Op>>
```

If using `EngineCtx` after base OCR is hard, create an equivalent context struct with:

```rust
scene
page
blobs
options
```

---

## Task 3 — Select suspicious nodes

For every text node:

1. Get current OCR text.
2. Read detector confidence from `TextData.confidence`.
3. Read OCR metadata:
   - `ocr_confidence`
   - `ocr_uncertain`
4. Re-run `assess_ocr_quality`.
5. Select node if:
   - `text.ocr_uncertain == true`
   - OR quality report says uncertain
   - OR text is empty
   - OR text has bad characters

Do not select nodes with good OCR.

---

## Task 4 — Batch-send only selected nodes

For selected nodes:

1. Load source image.
2. Crop selected text boxes.
3. Encode crops as PNG base64.
4. Send one `/ocr/crops` request.
5. Map response by node id.

No per-bubble HTTP loop.

---

## Task 5 — Update suspicious nodes only

For each successful response item, update node:

```rust
TextDataPatch {
    text: Some(Some(item.text.clone())),
    ocr_engine: Some(Some("unlimited-ocr".to_string())),
    ocr_confidence: Some(item.confidence),
    ocr_uncertain: Some(item.uncertain || quality_after.uncertain),
    translation_context: Some(Some(context_from_item)),
    ..Default::default()
}
```

If item missing:

```rust
TextDataPatch {
    ocr_uncertain: Some(true),
    ..Default::default()
}
```

Do not erase base OCR text if Unlimited-OCR result is missing or empty unless it is clearly better.

---

## Task 6 — Service failure behavior

In SmartFallback:

If Unlimited-OCR service is unavailable:

- Log warning.
- Keep base OCR text.
- Mark selected nodes `ocr_uncertain = true`.
- Do not fail pipeline if base OCR text exists.

If selected node has no base OCR text and service fails:

- Keep it empty.
- Mark `ocr_uncertain = true`.
- Add warning.
- Pipeline may continue to translation, but blank text should be skipped by translation.

---

## Task 7 — UI

Add or complete UI setting:

```text
Unlimited-OCR Mode:
- Off
- Smart fallback
- Full Unlimited-OCR
```

Add service URL field visible when mode is:

```text
Smart fallback
Full Unlimited-OCR
```

Default:

```text
http://127.0.0.1:7862
```

Recommended default for safe release:

```text
Off
```

Recommended default after testing:

```text
Smart fallback
```

---

## Task 8 — User-visible metadata

In text block panel, optionally show:

```text
OCR: manga-ocr
Uncertain: false
```

If fallback happened:

```text
OCR: unlimited-ocr
Uncertain: false
```

If suspicious:

```text
OCR: manga-ocr
Uncertain: true
```

---

## Task 9 — Logging

Use `tracing`.

Log:

```text
SmartFallback selected N of M boxes
Unlimited-OCR fallback succeeded for K boxes
Unlimited-OCR fallback failed: ...
```

Do not log full image base64.

---

## Tests

### Unit tests

Test node selection logic:

- good OCR not selected
- empty text selected
- bad chars selected
- low detector confidence selected
- already `ocr_uncertain = true` selected

### Integration tests with mock service

Test:

1. Good OCR:
   - service not called
2. Bad OCR:
   - service called once with selected node
3. Mixed page:
   - only bad nodes sent
4. Service fails:
   - base text kept
   - uncertain true
   - no panic
5. Full mode:
   - all nodes sent
6. Off mode:
   - service never called

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

Manual test:

```powershell
# Terminal 1
cd D:\project\koharu\services\unlimited-ocr
.\.venv\Scripts\activate
uvicorn server:app --host 127.0.0.1 --port 7862

# Terminal 2
cd D:\project\koharu
$env:UNLIMITED_OCR_URL="http://127.0.0.1:7862"
cargo run -p koharu -- --cpu --port 4000 --headless --debug

# Terminal 3
cd D:\project\koharu\ui
bun dev
```

---

## Acceptance Criteria

- SmartFallback mode runs base OCR first.
- Only suspicious boxes are sent to Unlimited-OCR.
- Good OCR boxes are not sent.
- Service failure does not crash SmartFallback if base OCR exists.
- Full mode still sends all boxes.
- Off mode never calls Unlimited-OCR.
- Translation uses final post-fallback OCR text.
- Existing 3 OCR engines still work.
