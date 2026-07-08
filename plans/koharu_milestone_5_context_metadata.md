# Milestone 5 — Translation Context Metadata & Data Model Completeness

## Goal

Fill the gaps between what was implemented in milestones 1–4 and the full master plan (`koharu_unlimited_ocr_context_translation_plan.md`).

No new features — only missing data model fields and wiring that were specified but not yet implemented.

---

## Scope

Implement:

1. `TextTranslationContext` struct + field in `TextData`
2. Patch support for `translation_context`
3. Extend `TranslationBlock` with all metadata fields
4. Update LLM system prompt to use new context fields
5. Map `translation_context` in unlimited-ocr engine + fallback

Do not implement:

- UI changes (Phase 7 — separate milestone)
- Benchmark doc (Phase 8 — separate milestone)
- Python `POST /ocr/page` endpoint (non-goal for v1)

---

## Files to inspect

```text
crates/koharu-core/src/scene.rs       — TextData, add TextTranslationContext
crates/koharu-core/src/op.rs          — TextDataPatch, capture_prev, apply
crates/koharu-app/src/pipeline/translation_context.rs  — TranslationBlock
crates/koharu-app/src/pipeline/engines/llm_translate.rs  — system prompt
crates/koharu-app/src/pipeline/engines/unlimited_ocr.rs  — map context
crates/koharu-app/src/pipeline/unlimited_ocr_fallback.rs  — map context
```

---

## Task 1 — Add `TextTranslationContext` struct

File: `crates/koharu-core/src/scene.rs`

Add after `TextData`:

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextTranslationContext {
    #[serde(default)]
    pub role: Option<String>,            // dialogue / narration / sfx / sign / unknown
    #[serde(default)]
    pub speaker_hint: Option<String>,
    #[serde(default)]
    pub emotion_hint: Option<String>,
    #[serde(default)]
    pub visual_hint: Option<String>,
    #[serde(default)]
    pub translation_note: Option<String>,
    #[serde(default)]
    pub context_uncertain: bool,
}
```

Add field to `TextData`:

```rust
    #[serde(default)]
    pub translation_context: Option<TextTranslationContext>,
```

Place after `ocr_uncertain`, before `text`.

Backward compatible: `#[serde(default)]` on all fields.

---

## Task 2 — Add field to `TextDataPatch`

File: `crates/koharu-core/src/op.rs`

Add after `ocr_uncertain`:

```rust
    #[serde(default)]
    pub translation_context: Option<Option<TextTranslationContext>>,
```

Add import: `TextTranslationContext` from `crate::scene`.

---

## Task 3 — Update `capture_prev_text`

Add:

```rust
    translation_context: p
        .translation_context
        .as_ref()
        .map(|_| data.translation_context.clone()),
```

Place after `ocr_uncertain` handling.

---

## Task 4 — Update `apply_text_patch`

Add:

```rust
    if let Some(v) = &p.translation_context {
        t.translation_context = v.clone();
    }
```

---

## Task 5 — Extend `TranslationBlock`

File: `crates/koharu-app/src/pipeline/translation_context.rs`

Add fields to `TranslationBlock`:

```rust
    pub ocr_engine: Option<String>,
    pub ocr_confidence: Option<f32>,
    pub ocr_uncertain: bool,
    pub role: Option<String>,
    pub speaker_hint: Option<String>,
    pub emotion_hint: Option<String>,
    pub visual_hint: Option<String>,
    pub translation_note: Option<String>,
```

Populate from `TextData` in `build_translation_blocks`:

```rust
    ocr_engine: td.ocr_engine.clone(),
    ocr_confidence: td.ocr_confidence,
    ocr_uncertain: td.ocr_uncertain,
    role: td.translation_context.as_ref().and_then(|c| c.role.clone()),
    speaker_hint: td.translation_context.as_ref().and_then(|c| c.speaker_hint.clone()),
    emotion_hint: td.translation_context.as_ref().and_then(|c| c.emotion_hint.clone()),
    visual_hint: td.translation_context.as_ref().and_then(|c| c.visual_hint.clone()),
    translation_note: td.translation_context.as_ref().and_then(|c| c.translation_note.clone()),
```

---

## Task 6 — Update LLM system prompt

File: `crates/koharu-app/src/pipeline/engines/llm_translate.rs`

In `translate_with_rich_context`, update the rich system prompt to mention the new context fields:

```text
You are a professional Japanese-to-Vietnamese manga translator.
Translate each Japanese manga text box naturally into Vietnamese.
Use reading order, previous/next text, position, and source direction as context.
If available, consider role (dialogue/narration/SFX), speaker hints, emotion hints, and visual hints as soft context.
If hints conflict with the Japanese text, prioritize the Japanese text.
Preserve tone, brevity, and manga style.
Return strict JSON only:
[
  {"id":"...", "translation":"..."}
]
```

---

## Task 7 — Map `translation_context` in unlimited-ocr engine

File: `crates/koharu-app/src/pipeline/engines/unlimited_ocr.rs`

When creating `TextDataPatch` from response items, add:

```rust
    translation_context: item.role.or(item.speaker_hint.as_ref())
        .map(|_| {
            Some(koharu_core::TextTranslationContext {
                role: item.role.clone(),
                speaker_hint: item.speaker_hint.clone(),
                emotion_hint: item.emotion_hint.clone(),
                visual_hint: item.visual_hint.clone(),
                translation_note: item.translation_note.clone(),
                context_uncertain: item.uncertain,
            })
        }),
```

If all context fields are `None`, use `None` (skip the patch entirely).
If at least one field is present, set `Some(Some(context))`.

---

## Task 8 — Map context in fallback

File: `crates/koharu-app/src/pipeline/unlimited_ocr_fallback.rs`

Same logic as Task 7 for the fallback path.

---

## Tests

### Core tests (`op.rs`)

- applying `translation_context` patch
- undo restores `translation_context`
- old JSON without `translation_context` loads

### Translation block tests (`translation_context.rs`)

- block includes `ocr_engine`, `ocr_uncertain`
- block includes `role`, `speaker_hint` from `translation_context`

---

## Commands

```powershell
cd D:\project\koharu
cargo test -p koharu-core
cargo test -p koharu-app
```

---

## Acceptance Criteria

- `TextTranslationContext` exists in scene model.
- `TextData` has `translation_context` field.
- Old project files load without error.
- `TranslationBlock` includes all metadata fields.
- LLM prompt mentions role/speaker/emotion/visual hints.
- Unlimited-OCR engine maps context fields from response.
- Fallback maps context fields from response.
- All tests pass.
