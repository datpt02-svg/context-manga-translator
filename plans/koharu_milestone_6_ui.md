# Milestone 6 — UI Unlimited-OCR Settings + OCR Metadata Display

## Goal

Add Unlimited-OCR mode dropdown, service URL input, and OCR metadata display to the Koharu UI.

---

## Scope

Implement:

1. Rust backend: `unlimited_ocr_mode` + `unlimited_ocr_url` in pipeline config
2. Rust RPC: fields in `StartPipelineRequest`
3. Rust pipeline driver: auto-select `unlimited-ocr` engine in Full mode
4. UI: Unlimited-OCR Mode dropdown in Settings → Engines
5. UI: Service URL input (visible when mode != Off)
6. UI: OCR metadata display in TextBlocksPanel
7. Regenerate OpenAPI + TS client

Do not implement:

- Service health check button (nice-to-have, skip for now)
- Benchmark doc

---

## Files to touch

### Rust backend

```text
crates/koharu-core/src/protocol.rs    — PipelineConfigPatch
crates/koharu-app/src/config.rs       — PipelineConfig + apply_patch
crates/koharu-app/src/pipeline/engine.rs  — PipelineRunOptions (already done)
crates/koharu-rpc/src/routes/pipelines.rs — StartPipelineRequest
crates/koharu-app/src/pipeline/mod.rs     — driver: Full mode routing
```

### UI

```text
ui/components/SettingsDialog.tsx       — dropdown + URL input
ui/components/panels/TextBlocksPanel.tsx — OCR metadata display
ui/public/locales/en/translation.json  (or similar) — i18n keys
```

### Generated

```text
ui/openapi.json
ui/lib/api/schemas/*
```

---

## Task 1 — Add unlimited-ocr config fields (Rust config)

File: `crates/koharu-core/src/protocol.rs`

Add to `PipelineConfigPatch`:

```rust
    #[serde(default)]
    pub unlimited_ocr_mode: Option<UnlimitedOcrMode>,
    #[serde(default)]
    pub unlimited_ocr_url: Option<Option<String>>,
```

Import `UnlimitedOcrMode` — it's in `koharu_app::pipeline::engine`. Since `koharu_core` cannot depend on `koharu_app`, move the enum to `koharu_core::protocol` OR keep it in `koharu_app` and use `Option<String>` for the patch field.

**Decision:** Move `UnlimitedOcrMode` to `koharu_core::protocol`. Re-export from `koharu_app::pipeline::engine` for backward compat.

---

## Task 2 — AppConfig PipelineConfig fields

File: `crates/koharu-app/src/config.rs`

Add to `PipelineConfig`:

```rust
    #[serde(default)]
    pub unlimited_ocr_mode: UnlimitedOcrMode,
    #[serde(default)]
    pub unlimited_ocr_url: Option<String>,
```

Update `Default` impl: mode = Off, url = None.

Update `apply_patch` to handle the new fields.

Update `validate_pipeline_config` if needed (soft validation — any value is valid).

---

## Task 3 — Wire to PipelineRunOptions

File: `crates/koharu-rpc/src/routes/pipelines.rs`

In `StartPipelineRequest`, add:

```rust
    #[serde(default)]
    pub unlimited_ocr_mode: UnlimitedOcrMode,
    #[serde(default)]
    pub unlimited_ocr_url: Option<String>,
```

In the `spec.options` construction, map:

```rust
    unlimited_ocr_mode: req.unlimited_ocr_mode,
    unlimited_ocr_url: req.unlimited_ocr_url,
```

---

## Task 4 — Pipeline driver: Full mode auto-select unlimited-ocr

File: `crates/koharu-app/src/pipeline/mod.rs`

In the pipeline `run()` function (or wherever `spec` is built into engine steps):

If `options.unlimited_ocr_mode == Full` and the step list contains a base OCR engine, replace it with `"unlimited-ocr"`.

Simpler approach: expose a helper function:

```rust
pub fn resolve_steps(options: &PipelineRunOptions, requested: &[String]) -> Vec<String>
```

That replaces the OCR step with `"unlimited-ocr"` when mode is Full.

If unsafe/confusing, document that the caller (RPC handler) should substitute the step. For now, do it at the RPC handler level for clarity.

---

## Task 5 — UI: Settings dropdown

File: `ui/components/SettingsDialog.tsx`

Add to the engines tab, below the OCR dropdown:

```tsx
<Label className='text-xs'>{t('settings.unlimitedOcrMode')}</Label>
<Select
  value={pipeline.unlimitedOcrMode ?? 'off'}
  onValueChange={(v) => onChange({ ...pipeline, unlimitedOcrMode: v })}
>
  <SelectTrigger className='w-full'><SelectValue /></SelectTrigger>
  <SelectContent>
    <SelectItem value='off'>Off</SelectItem>
    <SelectItem value='smart-fallback'>Smart fallback</SelectItem>
    <SelectItem value='full'>Full Unlimited-OCR</SelectItem>
  </SelectContent>
</Select>

{pipeline.unlimitedOcrMode !== 'off' && (
  <>
    <Label className='text-xs'>{t('settings.unlimitedOcrUrl')}</Label>
    <Input
      value={pipeline.unlimitedOcrUrl ?? 'http://127.0.0.1:7862'}
      onChange={(e) => onChange({ ...pipeline, unlimitedOcrUrl: e.target.value })}
      placeholder='http://127.0.0.1:7862'
    />
  </>
)}
```

Update `appConfigToPatch` to include new fields.

Update `UpdateConfigBody` type or the local state type to include:

```ts
unlimitedOcrMode?: string
unlimitedOcrUrl?: string | null
```

---

## Task 6 — UI: OCR metadata in TextBlocksPanel

File: `ui/components/panels/TextBlocksPanel.tsx`

In each accordion item (text block), below the OCR text label, add:

```tsx
<div className='flex gap-2 text-[10px] text-muted-foreground'>
  <span>OCR: {node.ocrEngine ?? '—'}</span>
  <span>Uncertain: {node.ocrUncertain ? '⚠ yes' : 'no'}</span>
</div>
```

This requires `TextData` TS type to have `ocrEngine` and `ocrUncertain`. Regenerate OpenAPI/TS after Rust changes.

---

## Task 7 — Regenerate

```powershell
cd D:\project\koharu
bun run generate:openapi
cd ui
bun run generate:api
```

Fix any TS type errors. Then:

```powershell
cd ui
bun run build
```

---

## Tests

- cargo check passes
- UI build passes
- Settings dialog renders new fields
- TextBlocksPanel shows OCR metadata

---

## Acceptance Criteria

- Settings → Engines shows Unlimited-OCR Mode dropdown with 3 options.
- Service URL input appears when mode != Off.
- Pipeline request includes `unlimitedOcrMode` and `unlimitedOcrUrl`.
- Full mode replaces OCR engine step with "unlimited-ocr".
- TextBlocksPanel shows OCR engine name + uncertainty per block.
- Build passes.
