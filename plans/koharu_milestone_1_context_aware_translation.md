# Milestone 1 — Context-Aware Translation

## Goal

Improve translation quality without adding Unlimited-OCR yet.

Keep all existing Koharu OCR engines unchanged:

- Manga OCR
- MIT 48px OCR
- PaddleOCR-VL

Current problem: LLM translation only receives plain OCR text strings. This milestone upgrades translation input to include layout/context metadata.

---

## Scope

Implement only:

1. Translation context builder
2. Rich translation block JSON
3. LLM translation using rich context
4. Safe fallback to old translation method

Do not implement:

- Python Unlimited-OCR service
- Rust Unlimited-OCR engine
- Smart fallback OCR
- New OCR mode UI
- OCR quality checker

---

## Files to inspect

```text
crates/koharu-app/src/pipeline/engines/llm_translate.rs
crates/koharu-app/src/pipeline/engines/support.rs
crates/koharu-core/src/scene.rs
crates/koharu-app/src/llm/
crates/koharu-app/src/pipeline/engine.rs
```

---

## Task 1 — Add translation context builder

Create:

```text
crates/koharu-app/src/pipeline/translation_context.rs
```

Add type:

```rust
#[derive(Debug, Clone, serde::Serialize)]
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
    pub previous_text: Option<String>,
    pub next_text: Option<String>,
}
```

Add function:

```rust
pub fn build_translation_blocks(
    scene: &Scene,
    page: PageId,
    allowed_ids: Option<&[NodeId]>,
    reading_order: Option<ReadingOrder>,
) -> Vec<(NodeId, TranslationBlock)>
```

Behavior:

- Use existing text nodes.
- Skip empty OCR text.
- Preserve `allowed_ids` filtering.
- Compute bbox from node transform:
  - `[x, y, x + width, y + height]`
- Compute `position` from page size:
  - `top-left`
  - `top-center`
  - `top-right`
  - `middle-left`
  - `middle-center`
  - `middle-right`
  - `bottom-left`
  - `bottom-center`
  - `bottom-right`
- Fill `previous_text` and `next_text` based on reading order.
- If reading order sort is hard, use current text node order for v1 and add TODO.

---

## Task 2 — Update module exports

Find pipeline module file, likely:

```text
crates/koharu-app/src/pipeline/mod.rs
```

Add:

```rust
pub mod translation_context;
```

---

## Task 3 — Update LLM translation engine

File:

```text
crates/koharu-app/src/pipeline/engines/llm_translate.rs
```

Current behavior collects plain strings. Replace with rich blocks.

Desired flow:

```rust
let targets = build_translation_blocks(
    ctx.scene,
    ctx.page,
    ctx.options.text_node_ids.as_deref(),
    ctx.options.reading_order,
);

let blocks: Vec<TranslationBlock> = targets.iter().map(|(_, b)| b.clone()).collect();

let translations = ctx.llm
    .translate_text_blocks(
        &blocks,
        ctx.options.target_language.as_deref(),
        ctx.options.system_prompt.as_deref(),
    )
    .await;
```

If implementing `translate_text_blocks` is too invasive, create a wrapper that serializes the blocks into one structured prompt and uses existing LLM call internals.

---

## Task 4 — Add rich translation prompt

The LLM prompt should say:

```text
You are a professional Japanese-to-Vietnamese manga translator.
Translate each Japanese manga text box naturally into Vietnamese.
Use reading order, previous/next text, position, and source direction as context.
Preserve tone, brevity, and manga style.
Return strict JSON only:
[
  {"id":"...", "translation":"..."}
]
```

Input payload shape:

```json
{
  "targetLanguage": "Vietnamese",
  "blocks": [
    {
      "id": "node-id",
      "text": "何してるの？",
      "order": 1,
      "bbox": [820, 40, 1020, 260],
      "position": "top-right",
      "sourceDirection": "vertical",
      "detectorConfidence": 0.91,
      "detector": "ctd-full",
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

---

## Task 5 — Output mapping

Map translations by `id`.

Fallback rules:

1. If JSON parse succeeds and ids match, use id mapping.
2. If JSON parse fails, fallback to old `translate_texts` behavior.
3. If JSON parse succeeds but some ids are missing:
   - if output length equals input length, fallback to index mapping
   - otherwise skip missing ids and log warning

---

## Task 6 — Preserve old behavior

Do not remove existing `translate_texts` path.

Rich context translation must fail gracefully:

```text
rich context translation failed → old plain text translation fallback
```

---

## Tests

Add unit tests for `translation_context.rs`:

- skip blank OCR text
- include bbox
- compute position correctly
- include previous/next text
- respect allowed `text_node_ids`

Add/update tests for `llm_translate.rs`:

- requested nodes only
- blank text ignored
- id mapping works
- fallback to old path on JSON parse error if feasible

---

## Commands

```powershell
cd D:\project\koharu
cargo test -p koharu-app
cargo test -p koharu-core
cd ui
bun run build
```

---

## Acceptance Criteria

- Existing 3 OCR engines still work.
- Translation now receives structured context, not only raw strings.
- User-visible translation quality should improve even without Unlimited-OCR.
- No Python service required.
- No Unlimited-OCR setting required.
- Old translation path still exists as fallback.
