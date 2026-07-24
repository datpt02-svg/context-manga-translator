//! Koharu renderer engine. Rasterises each text node's translation into an
//! RGBA sprite, composites them onto the inpainted plane, and writes back:
//!
//! - per-block `UpdateNode { TextDataPatch { sprite, sprite_transform,
//!   rendered_direction, style } }` (sprite blob stored as raw RGBA)
//! - one `upsert Image { role: Rendered }` for the final composite (webp)
//!
//! Requires an `Image { role: Inpainted }` node on the page.

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use koharu_core::{
    ImageRole, MaskRole, NodeDataPatch, NodeId, NodeKind, NodePatch, Op, TextDataPatch, TextStyle,
    Transform,
};
use koharu_llm::Language;

use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo};
use crate::pipeline::engines::support::{
    find_image_node, find_mask_node, image_dimensions, load_source_image, text_nodes,
    upsert_image_blob,
};
use koharu_renderer::text::latin::{BubbleIndex, LayoutBox};
use koharu_renderer::text::script::writing_mode_for_block;
use koharu_renderer::types::RenderBlock;

use crate::renderer::{PageRenderOptions, RenderBlockInput};

pub struct Model;

#[async_trait]
impl Engine for Model {
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>> {
        // Find the target surface: prefer inpainted, fall back to source.
        let base = match find_image_node(ctx.scene, ctx.page, ImageRole::Inpainted) {
            Some((_, blob)) => ctx.blobs.load_image(&blob)?,
            None => load_source_image(ctx.scene, ctx.page, ctx.blobs)?,
        };
        let (w, h) = image_dimensions(&base);

        // Brush layer (optional): overlay before text sprites.
        let brush = match find_mask_node(ctx.scene, ctx.page, MaskRole::BrushInpaint) {
            Some((_, blob)) => Some(ctx.blobs.load_image(&blob)?),
            None => None,
        };

        // Bubble-interior mask (optional): grows latin layout boxes so text
        // wraps inside the available bubble space.
        let bubble = match find_mask_node(ctx.scene, ctx.page, MaskRole::Bubble) {
            Some((_, blob)) => Some(ctx.blobs.load_image(&blob)?),
            None => None,
        };

        // Build renderer input from every text node with a non-empty translation.
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

        let page_opts = PageRenderOptions {
            shader_effect: Default::default(),
            shader_stroke: None,
            document_font: ctx.options.default_font.clone(),
            target_language: ctx
                .options
                .target_language
                .as_deref()
                .map(render_target_language_tag),
            raster: Default::default(),
        };

        // `render_page` is synchronous and CPU-bound. It runs inline on the
        // current tokio worker; for multi-page jobs the driver parallelises
        // across pages via separate `run()` calls.
        let output = ctx.renderer.render_page(
            &base,
            brush.as_ref(),
            bubble.as_ref(),
            w,
            h,
            &inputs,
            &page_opts,
        )?;

        // ---- Detect overflow + shorten via LLM ----
        // When a rendered sprite exceeds its text box, call the LLM to
        // shorten the translation, patch the scene node, and re-render.
        let mut shorten_map: HashMap<NodeId, String> = HashMap::new();
        for block_out in &output.blocks {
            let Some(input) = inputs.iter().find(|i| i.node_id == block_out.node_id) else {
                continue;
            };
            if !should_shorten_rendered_block(block_out.fits) {
                continue;
            }
            let source_text =
                find_source_text(ctx.scene, ctx.page, block_out.node_id).unwrap_or_default();
            let prompt = serde_json::json!({
                "sourceText": source_text,
                "translation": &input.translation,
                "boxWidth": input.transform.width as u32,
                "boxHeight": input.transform.height as u32,
            });
            let system = "\
                You are a manga text shortening assistant.\n\
                Shorten the given translation to fit inside the text box \
                (width x height in px).\n\
                Preserve meaning, tone, key names and numbers.\n\
                Keep it natural — do not just remove words.\n\
                Return ONLY the shortened text, no markdown, no JSON wrapper.";
            match ctx
                .llm
                .translate_raw(&prompt.to_string(), Some(system), None)
                .await
            {
                Ok(shortened) => {
                    let s = shortened.trim().to_string();
                    if !s.is_empty() && s != input.translation {
                        tracing::info!(
                            node = %block_out.node_id,
                            before = %input.translation,
                            after = %s,
                            "LLM-shortened overflowed text"
                        );
                        shorten_map.insert(block_out.node_id, s);
                    }
                }
                Err(e) => {
                    tracing::warn!(node = %block_out.node_id, "shorten LLM failed: {e:#}");
                }
            }
        }

        // If any blocks were shortened, re-render with the new translations.
        let (sprites, final_render) = if shorten_map.is_empty() {
            (output.blocks, output.final_render)
        } else {
            let mut short_inputs = inputs.clone();
            for block in &mut short_inputs {
                if let Some(s) = shorten_map.get(&block.node_id) {
                    block.translation = s.clone();
                }
            }
            match ctx.renderer.render_page(
                &base,
                brush.as_ref(),
                bubble.as_ref(),
                w,
                h,
                &short_inputs,
                &page_opts,
            ) {
                Ok(short_out) => (short_out.blocks, short_out.final_render),
                Err(e) => {
                    tracing::warn!("re-render after shorten failed: {e:#}");
                    (output.blocks, output.final_render)
                }
            }
        };

        // Upload sprites + compose ops.
        // Pre-compute bubble contours so each block op can include its outline.
        let bubble_contours = build_bubble_contour_map(bubble.as_ref(), &nodes);

        let mut ops = Vec::with_capacity(sprites.len() + 1);
        for block_out in sprites {
            let sprite_ref = ctx.blobs.put_raw(&block_out.sprite)?;
            let existing_style = inputs
                .iter()
                .find(|i| i.node_id == block_out.node_id)
                .and_then(|i| i.style.clone());
            let shortened_translation =
                shortened_translation_patch(&shorten_map, block_out.node_id);
            let contour = bubble_contours.get(&block_out.node_id).cloned();
            ops.push(Op::UpdateNode {
                page: ctx.page,
                id: block_out.node_id,
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(TextDataPatch {
                        sprite: Some(Some(sprite_ref)),
                        sprite_transform: Some(
                            block_out.expanded_transform.map(normalize_transform),
                        ),
                        rendered_direction: Some(Some(block_out.rendered_direction)),
                        // Only persist explicit user style overrides. Writing
                        // a synthetic default style back into the scene makes
                        // later renders treat implicit predicted colors as
                        // explicit black overrides.
                        style: preserve_existing_style(existing_style),
                        translation: shortened_translation,
                        bubble_contour: Some(contour),
                        ..Default::default()
                    })),
                    transform: None,
                    visible: None,
                },
                prev: NodePatch::default(),
            });
        }

        // Final composite → Image { Rendered } upsert.
        let final_blob = ctx.blobs.put_webp(&final_render)?;
        ops.push(upsert_image_blob(
            ctx.scene,
            ctx.page,
            ImageRole::Rendered,
            final_blob,
            w,
            h,
        ));
        Ok(ops)
    }
}

inventory::submit! {
    EngineInfo {
        id: "koharu-renderer",
        name: "Koharu Renderer",
        needs: &[
            Artifact::Inpainted,
            Artifact::Translations,
            Artifact::FontPredictions,
        ],
        produces: &[Artifact::FinalRender, Artifact::RenderedSprites],
        load: |_runtime, _cpu| Box::pin(async move {
            Ok(Box::new(Model) as Box<dyn Engine>)
        }),
    }
}

fn normalize_transform(t: Transform) -> Transform {
    Transform {
        x: t.x.round(),
        y: t.y.round(),
        width: t.width.round(),
        height: t.height.round(),
        rotation_deg: t.rotation_deg,
    }
}

fn preserve_existing_style(existing: Option<TextStyle>) -> Option<Option<TextStyle>> {
    existing.map(Some)
}

fn shortened_translation_patch(
    shorten_map: &HashMap<NodeId, String>,
    node_id: NodeId,
) -> Option<Option<String>> {
    shorten_map.get(&node_id).cloned().map(Some)
}

fn should_shorten_rendered_block(fits: bool) -> bool {
    !fits
}

/// Look up the OCR source text for a text node.
fn find_source_text(
    scene: &koharu_core::Scene,
    page: koharu_core::PageId,
    node_id: NodeId,
) -> Option<String> {
    let page_ref = scene.page(page)?;
    let node = page_ref.nodes.get(&node_id)?;
    match &node.kind {
        NodeKind::Text(t) => t.text.clone(),
        _ => None,
    }
}

fn render_target_language_tag(value: &str) -> String {
    Language::parse(value)
        .map(|language| language.tag().to_string())
        .unwrap_or_else(|| value.to_string())
}

fn build_bubble_contour_map(
    bubble: Option<&image::DynamicImage>,
    nodes: &[(NodeId, &koharu_core::Transform, &koharu_core::TextData)],
) -> HashMap<NodeId, Vec<[f32; 2]>> {
    let Some(bubble_img) = bubble else {
        return HashMap::new();
    };
    let gray = bubble_img.to_luma8();
    let index = BubbleIndex::new(gray.clone());
    let mut map = HashMap::new();

    for &(node_id, transform, text_data) in nodes {
        let render_block = RenderBlock {
            x: transform.x,
            y: transform.y,
            width: transform.width.max(1.0),
            height: transform.height.max(1.0),
            text: text_data.translation.as_deref().unwrap_or("a").to_string(),
            source_direction: text_data.source_direction.map(|d| match d {
                koharu_core::TextDirection::Horizontal => {
                    koharu_renderer::types::TextDirection::Horizontal
                }
                koharu_core::TextDirection::Vertical => {
                    koharu_renderer::types::TextDirection::Vertical
                }
            }),
        };
        let writing_mode = writing_mode_for_block(&render_block);
        let seed = LayoutBox {
            x: transform.x,
            y: transform.y,
            width: transform.width.max(1.0),
            height: transform.height.max(1.0),
        };

        if let Some(matched) = index.lookup_match(seed, writing_mode) {
            if let Some(contour) =
                koharu_ml::bubble_contour::extract_contour_from_id_mask(&gray, matched.id)
            {
                map.insert(node_id, contour);
            }
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{preserve_existing_style, render_target_language_tag, shortened_translation_patch};
    use koharu_core::{NodeId, TextStyle};

    #[test]
    fn omits_style_patch_when_block_has_no_explicit_style() {
        assert!(preserve_existing_style(None).is_none());
    }

    #[test]
    fn preserves_existing_explicit_style() {
        let style = TextStyle {
            font_families: vec!["Arial".to_string()],
            font_size: Some(18.0),
            color: [12, 34, 56, 255],
            effect: None,
            stroke: None,
            text_align: None,
        };
        let preserved = preserve_existing_style(Some(style));
        let Some(Some(preserved)) = preserved else {
            panic!("expected explicit style patch");
        };
        assert_eq!(preserved.font_families, vec!["Arial".to_string()]);
        assert_eq!(preserved.font_size, Some(18.0));
        assert_eq!(preserved.color, [12, 34, 56, 255]);
        assert!(preserved.effect.is_none());
        assert!(preserved.stroke.is_none());
        assert!(preserved.text_align.is_none());
    }

    #[test]
    fn render_target_language_normalizes_language_names() {
        assert_eq!(render_target_language_tag("German"), "de-DE");
        assert_eq!(render_target_language_tag("pt-BR"), "pt-BR");
        assert_eq!(
            render_target_language_tag("not-a-language"),
            "not-a-language"
        );
    }

    #[test]
    fn shortened_translation_patch_only_patches_shortened_nodes() {
        let shortened_id = NodeId::new();
        let untouched_id = NodeId::new();
        let mut shorten_map = HashMap::new();
        shorten_map.insert(shortened_id, "Short text".to_string());

        assert_eq!(
            shortened_translation_patch(&shorten_map, shortened_id),
            Some(Some("Short text".to_string()))
        );
        assert_eq!(
            shortened_translation_patch(&shorten_map, untouched_id),
            None
        );
    }

    #[test]
    fn shorten_trigger_uses_fit_metadata() {
        assert!(super::should_shorten_rendered_block(false));
        assert!(!super::should_shorten_rendered_block(true));
    }
}
