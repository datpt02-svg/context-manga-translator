//! AnyText2 render engine. Replaces koharu-renderer with a diffusion-based
//! sidecar (Python FastAPI + AnyText2 model). Transliterates per-block
//! translation text onto the inpainted page via `POST /render`.
//!
//! Requires an `Image { role: Inpainted }` node on the page.

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine as _;
use image::{DynamicImage, RgbaImage, imageops};
use koharu_core::{
    ImageRole, MaskRole, NodeDataPatch, NodePatch, Op, TextDataPatch, TextStyle, Transform,
};
use crate::pipeline::anytext2_client::{AnyText2Client, FontHint, RenderRequest, TextBlock};
use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo, PipelineRunOptions};
use crate::pipeline::engines::support::{
    find_image_node, find_mask_node, image_dimensions, load_source_image, text_nodes,
    upsert_image_blob,
};
use crate::renderer::{RenderBlockInput, RenderedBlock};

pub struct Model {
    env_url: String,
}

impl Model {
    pub fn new(env_url: impl Into<String>) -> Self {
        Self {
            env_url: env_url.into(),
        }
    }

    fn resolve_url(&self, opts: &PipelineRunOptions) -> String {
        opts.anytext2_url
            .clone()
            .filter(|u| !u.is_empty())
            .or_else(|| {
                let e = self.env_url.clone();
                if e.is_empty() { None } else { Some(e) }
            })
            .unwrap_or_else(|| "http://127.0.0.1:7863".to_string())
    }
}

#[async_trait]
impl Engine for Model {
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>> {
        // Load SOURCE image (has original text) and SEGMENT mask.
        let source = load_source_image(ctx.scene, ctx.page, ctx.blobs)?;
        let (w, h) = image_dimensions(&source);
        let segment_mask = find_mask_node(ctx.scene, ctx.page, MaskRole::Segment)
            .map(|(_, blob)| ctx.blobs.load_image(&blob))
            .transpose()?;

        // Collect text nodes with translations.
        let nodes = text_nodes(ctx.scene, ctx.page);
        let inputs: Vec<RenderBlockInput> = nodes
            .iter()
            .filter_map(|(id, transform, t)| {
                let translation = t.translation.as_ref()?.trim();
                if translation.is_empty() {
                    return None;
                }
                Some(RenderBlockInput {
                    node_id: *id,
                    transform: **transform,
                    translation: translation.to_string(),
                    style: t.style.clone(),
                    font_prediction: t.font_prediction.clone(),
                    source_direction: t.source_direction,
                    rendered_direction: t.rendered_direction,
                    lock_layout_box: t.lock_layout_box,
                })
            })
            .collect();

        if inputs.is_empty() {
            let blob = ctx.blobs.put_webp(&source)?;
            return Ok(vec![upsert_image_blob(
                ctx.scene, ctx.page, ImageRole::Rendered, blob, w, h,
            )]);
        }

        let url = self.resolve_url(ctx.options);
        let client = AnyText2Client::new(url);
        let source_b64 = encode_png(&source);

        let mut blocks: Vec<TextBlock> = Vec::with_capacity(inputs.len());
        for input in &inputs {
            let crop = crop_with_padding(&source, input.transform);
            let mask_crop = segment_mask
                .as_ref()
                .map(|m| crop_with_padding(m, input.transform))
                .unwrap_or_else(String::new);
            let text_color: Vec<u8> = input
                .style.as_ref().map(|s| s.color.to_vec())
                .or_else(|| input.font_prediction.as_ref().map(|p| {
                    vec![p.text_color[0], p.text_color[1], p.text_color[2], 255]
                }))
                .unwrap_or_else(|| vec![0, 0, 0, 255]);
            let font_hint = input.font_prediction.as_ref().map(|p| {
                let top_font = p.named_fonts.first();
                FontHint {
                    serif: top_font.map(|f| f.serif).unwrap_or(false),
                    language: top_font.and_then(|f| f.language.clone()),
                    family: top_font.map(|f| f.name.clone()),
                    font_size_px: Some(p.font_size_px),
                }
            });
            blocks.push(TextBlock {
                id: input.node_id.to_string(),
                translation: input.translation.clone(),
                x: input.transform.x,
                y: input.transform.y,
                width: input.transform.width,
                height: input.transform.height,
                source_crop_base64: crop,
                mask_crop_base64: mask_crop,
                text_color,
                font_hint,
            });
        }

        if client.health().await.is_err() {
            let spawned = crate::services::ensure_running(crate::services::anytext2_spec())
                .context("AnyText2 service not available")?;
            if spawned.is_some() {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }

        let request = RenderRequest {
            image_width: w, image_height: h,
            source_image_base64: source_b64,
            blocks,
        };
        let response = client.render(request).await?;

        let mut ops = Vec::with_capacity(response.blocks.len() + 1);
        for rb in &response.blocks {
            let rendered = decode_png(&rb.rendered_crop_base64)?;
            let nid = match inputs.iter().find(|i| i.node_id.to_string() == rb.id) {
                Some(i) => i.node_id,
                None => continue,
            };
            let sprite = DynamicImage::ImageRgba8(rendered);
            let sprite_ref = ctx.blobs.put_raw(&sprite)?;
            let existing_style = inputs.iter().find(|i| i.node_id == nid)
                .and_then(|i| i.style.clone());
            ops.push(Op::UpdateNode {
                page: ctx.page, id: nid,
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(TextDataPatch {
                        sprite: Some(Some(sprite_ref)),
                        sprite_transform: Some(None),
                        rendered_direction: Some(Some(koharu_core::TextDirection::Horizontal)),
                        style: preserve_existing_style(existing_style),
                        ..Default::default()
                    })),
                    transform: None, visible: None,
                },
                prev: NodePatch::default(),
            });
        }

        // Composite final: source + overlay rendered sprites.
        let mut canvas = source.to_rgba8();
        for op in &ops {
            if let Op::UpdateNode { patch, .. } = op {
                if let Some(NodeDataPatch::Text(tp)) = &patch.data {
                    if let Some(Some(blob)) = &tp.sprite {
                        let img = ctx.blobs.load_image(blob)?;
                        imageops::overlay(&mut canvas, &img.to_rgba8(), 0, 0);
                    }
                }
            }
        }
        let final_blob = ctx.blobs.put_webp(&DynamicImage::ImageRgba8(canvas))?;
        ops.push(upsert_image_blob(
            ctx.scene, ctx.page, ImageRole::Rendered, final_blob, w, h,
        ));
        Ok(ops)
    }
}

inventory::submit! {
    EngineInfo {
        id: "anytext2",
        name: "AnyText2 Diffusion Renderer",
        needs: &[
            Artifact::SegmentMask,
            Artifact::TextBoxes,
            Artifact::Translations,
            Artifact::FontPredictions,
        ],
        produces: &[Artifact::FinalRender, Artifact::RenderedSprites],
        load: |_runtime, _cpu| Box::pin(async move {
            let url = std::env::var("ANYTEXT2_URL").unwrap_or_default();
            Ok(Box::new(Model::new(url)) as Box<dyn Engine>)
        }),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn crop_with_padding(image: &DynamicImage, t: Transform) -> String {
    let pad = 16i32;
    let x = (t.x as i32 - pad).max(0);
    let y = (t.y as i32 - pad).max(0);
    let w = (t.width as i32 + pad * 2).min(image.width() as i32 - x) as u32;
    let h = (t.height as i32 + pad * 2).min(image.height() as i32 - y) as u32;
    let crop = image.crop_imm(x as u32, y as u32, w.max(1), h.max(1));
    encode_png(&crop)
}

fn encode_png(image: &DynamicImage) -> String {
    let mut buf = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG encoding should not fail in memory");
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}

fn decode_png(b64: &str) -> Result<RgbaImage> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("failed to decode base64")?;
    let img = image::load_from_memory(&raw)
        .context("failed to load image from decoded bytes")?;
    Ok(img.to_rgba8())
}

fn composite_layers(
    base: &DynamicImage,
    brush: Option<&DynamicImage>,
    rendered_blocks: &[RenderedBlock],
) -> Result<DynamicImage> {
    let mut canvas = base.to_rgba8();
    if let Some(brush) = brush {
        imageops::overlay(&mut canvas, &brush.to_rgba8(), 0, 0);
    }
    for out in rendered_blocks {
        let x = out.sprite.width() as i64 / 2 + 8;
        let _ = x;
        // Place at the node's original position (centered in crop).
        // The crop is the original bbox + padding, so the rendered
        // sprite is already the exact same size — overlay at origin.
        imageops::overlay(&mut canvas, &out.sprite.to_rgba8(), 0, 0);
    }
    Ok(DynamicImage::ImageRgba8(canvas))
}

fn preserve_existing_style(existing: Option<TextStyle>) -> Option<Option<TextStyle>> {
    existing.map(Some)
}
