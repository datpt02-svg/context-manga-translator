//! Koharu text renderer.
//!
//! Owns the font book, symbol fallbacks, and Google Fonts service. Exposes
//! [`Renderer::render_page`], which rasterises each text block's translation
//! into an RGBA sprite and composites them onto the inpainted plane.
//!
//! Pure output: the pipeline engine ([`crate::pipeline::engines::renderer`])
//! takes a `RenderOutput` and translates sprites + final composite into ops.

#[cfg(test)]
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
#[cfg(test)]
use image::GrayImage;
use image::{DynamicImage, RgbaImage, imageops};
use koharu_core::{
    FontFaceInfo, FontPrediction, FontSource, NodeId, TextDirection, TextShaderEffect,
    TextStrokeStyle, TextStyle, Transform,
};

use koharu_renderer::{
    TextAlign as RendererTextAlign, TextShaderEffect as RendererEffect,
    font::{FaceInfo, Font, FontBook},
    layout::{LayoutRun, TextLayout, WritingMode},
    renderer::{RasterOptions, RenderOptions, RenderStrokeOptions, TinySkiaRenderer},
    text::{
        latin::LayoutBox,
        script::{font_families_for_text, writing_mode_for_block},
    },
    types::{RenderBlock, TextDirection as RendererTextDirection},
};

#[cfg(test)]
use koharu_renderer::text::latin::BubbleIndex;

use crate::google_fonts::GoogleFontService;

// ---------------------------------------------------------------------------
// Inputs / outputs
// ---------------------------------------------------------------------------

/// Per-block input (immutable snapshot of a scene text node).
#[derive(Debug, Clone)]
pub struct RenderBlockInput {
    pub node_id: NodeId,
    pub transform: Transform,
    pub translation: String,
    pub style: Option<TextStyle>,
    pub font_prediction: Option<FontPrediction>,
    pub source_direction: Option<TextDirection>,
    pub rendered_direction: Option<TextDirection>,
    pub lock_layout_box: bool,
}

/// Document-level render options (shared across all blocks).
#[derive(Debug, Clone, Default)]
pub struct PageRenderOptions {
    pub shader_effect: TextShaderEffect,
    pub shader_stroke: Option<TextStrokeStyle>,
    pub document_font: Option<String>,
    pub target_language: Option<String>,
    pub raster: RasterOptions,
}

/// Per-block sprite output. `expanded_transform` is persisted as
/// `TextData.sprite_transform`; in the strict renderer it describes the clipped
/// sprite placement/dimensions inside the original text box.
pub struct RenderedBlock {
    pub node_id: NodeId,
    pub sprite: DynamicImage,
    pub rendered_direction: TextDirection,
    pub expanded_transform: Option<Transform>,
    pub fits: bool,
    pub font_size_px: f32,
}

/// Result of rendering a whole page.
pub struct RenderOutput {
    pub final_render: DynamicImage,
    pub blocks: Vec<RenderedBlock>,
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

pub struct Renderer {
    fontbook: Arc<Mutex<FontBook>>,
    renderer: TinySkiaRenderer,
    symbol_fallbacks: Vec<Font>,
    pub google_fonts: Arc<GoogleFontService>,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        let mut fontbook = FontBook::new();
        let symbol_fallbacks = load_symbol_fallbacks(&mut fontbook);
        let app_data_root = koharu_runtime::default_app_data_root();
        let google_fonts = Arc::new(
            GoogleFontService::new(&app_data_root)
                .context("failed to initialize Google Fonts service")?,
        );
        Ok(Self {
            fontbook: Arc::new(Mutex::new(fontbook)),
            renderer: TinySkiaRenderer::new()?,
            symbol_fallbacks,
            google_fonts,
        })
    }

    /// List system + cached Google Fonts for the API.
    pub fn available_fonts(&self) -> Result<Vec<FontFaceInfo>> {
        let fontbook = self
            .fontbook
            .lock()
            .map_err(|_| anyhow::anyhow!("failed to lock fontbook"))?;
        let mut fonts = fontbook
            .all_families()
            .into_iter()
            .filter(|face| !face.post_script_name.is_empty())
            .map(|face| {
                let family_name = face
                    .families
                    .first()
                    .map(|(family, _)| family.clone())
                    .unwrap_or_else(|| face.post_script_name.clone());
                FontFaceInfo {
                    family_name,
                    post_script_name: face.post_script_name,
                    source: FontSource::System,
                    category: None,
                    cached: true,
                }
            })
            .collect::<Vec<_>>();
        let catalog = self.google_fonts.catalog();
        for entry in &catalog.fonts {
            for variant in &entry.variants {
                // Unique PS name for Google Fonts to identify the specific weight/style
                let post_script_name = format!(
                    "{}:{}{}",
                    entry.family,
                    variant.weight,
                    if variant.style == "italic" { "i" } else { "" }
                );

                fonts.push(FontFaceInfo {
                    family_name: entry.family.clone(),
                    post_script_name,
                    source: FontSource::Google,
                    category: Some(entry.category.clone()),
                    cached: self.google_fonts.is_variant_cached(&entry.family, variant),
                });
            }
        }
        fonts.sort();
        Ok(fonts)
    }

    /// Render every block's translation, composite onto `inpainted`, return
    /// the full page + per-block sprites. Blocks with an empty translation
    /// are skipped (they appear as holes in the composite, falling through to
    /// the inpainted plane).
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(level = "info", skip_all, fields(blocks = blocks.len()))]
    pub fn render_page(
        &self,
        inpainted: &DynamicImage,
        brush_layer: Option<&DynamicImage>,
        _bubble_mask: Option<&DynamicImage>,
        image_width: u32,
        image_height: u32,
        blocks: &[RenderBlockInput],
        opts: &PageRenderOptions,
    ) -> Result<RenderOutput> {
        let min_font = min_font_size_for_image(image_width, image_height);
        let page_style = self.resolve_page_style_template(blocks, opts);
        let line_height = resolve_page_line_height(blocks);

        let mut rendered_blocks = Vec::with_capacity(blocks.len());
        for block in blocks {
            match self.render_one(
                block,
                seed_layout_box(block),
                &page_style,
                line_height,
                opts.target_language.as_deref(),
                opts.raster,
                min_font,
            ) {
                Ok(Some(out)) => rendered_blocks.push(out),
                Ok(None) => {}
                Err(e) => tracing::warn!(node = %block.node_id, "render failed: {e:#}"),
            }
        }

        // Compose the final page: inpainted → brush → per-block sprites.
        let mut canvas = inpainted.to_rgba8();
        if let Some(brush) = brush_layer {
            imageops::overlay(&mut canvas, &brush.to_rgba8(), 0, 0);
        }
        for out in &rendered_blocks {
            let (x, y) = placement_origin(find_input(blocks, out.node_id), &out.expanded_transform);
            imageops::overlay(&mut canvas, &out.sprite.to_rgba8(), x as i64, y as i64);
        }
        Ok(RenderOutput {
            final_render: DynamicImage::ImageRgba8(canvas),
            blocks: rendered_blocks,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_one(
        &self,
        block: &RenderBlockInput,
        layout_box: LayoutBox,
        page_style: &TextStyle,
        line_height: f32,
        target_language: Option<&str>,
        raster: RasterOptions,
        min_font_size: f32,
    ) -> Result<Option<RenderedBlock>> {
        let translation = block.translation.trim();
        if translation.is_empty() {
            return Ok(None);
        }

        let layout_source = layout_source_from_input(block, translation);
        let style = page_style.clone();
        let font_size_hint = font_size_hint(block);
        let font = self.select_font(&style)?;
        let block_effect = style.effect.unwrap_or_default();
        let color = style.color;

        let writing_mode = writing_mode_for_block(&layout_source);
        let align = style
            .text_align
            .map(core_align_to_renderer)
            .unwrap_or(RendererTextAlign::Center);
        let block_min_font = min_font_size_for_box(layout_box, min_font_size);

        let mut layout_builder = TextLayout::new(&font, None)
            .with_fallback_fonts(&self.symbol_fallbacks)
            .with_writing_mode(writing_mode)
            .with_alignment(align)
            .with_line_height_scale(line_height);
        if let Some(target_language) = target_language {
            layout_builder = layout_builder.with_hyphenation_language_tag(target_language);
        }
        let max_font = max_font_size_for_box(layout_box, min_font_size);
        let mut render_candidate = |layout: &LayoutRun<'_>| -> Result<RenderedTextCandidate> {
            let resolved_stroke =
                resolve_stroke_style(None, style.stroke.as_ref(), None, layout.font_size, color);

            let rendered = self.renderer.render(
                layout,
                writing_mode,
                &RenderOptions {
                    font_size: layout.font_size,
                    color,
                    effect: shader_core_to_renderer(block_effect),
                    stroke: resolved_stroke,
                    raster,
                    ..Default::default()
                },
            )?;
            let transform = centred_sprite_transform(
                layout_box,
                rendered.width(),
                rendered.height(),
                block.transform.rotation_deg,
            );
            Ok(RenderedTextCandidate {
                image: rendered,
                transform,
            })
        };

        let candidate = fit_rendered_text(
            &layout_builder,
            translation,
            layout_box,
            font_size_hint,
            block_min_font,
            max_font,
            &mut render_candidate,
        )?;
        let clipped = clip_to_box(
            DynamicImage::ImageRgba8(candidate.image),
            layout_box,
            &candidate.transform,
        );
        let fits = candidate.fits && !clipped.was_clipped;

        Ok(Some(RenderedBlock {
            node_id: block.node_id,
            sprite: clipped.image,
            rendered_direction: rendered_direction_for_writing_mode(writing_mode),
            expanded_transform: Some(clipped.transform),
            fits,
            font_size_px: candidate.font_size,
        }))
    }

    fn resolve_page_style_template(
        &self,
        blocks: &[RenderBlockInput],
        opts: &PageRenderOptions,
    ) -> TextStyle {
        let mut style = blocks
            .iter()
            .find_map(|block| block.style.clone())
            .unwrap_or_default();
        style.font_size = None;

        if style.effect.is_none() {
            style.effect = Some(opts.shader_effect);
        }
        if style.stroke.is_none() {
            style.stroke = opts.shader_stroke.clone();
        }
        if style.text_align.is_none() {
            style.text_align = Some(koharu_core::TextAlign::Center);
        }
        if style.color == [0, 0, 0, 255]
            && let Some(pred) = blocks
                .iter()
                .find_map(|block| block.font_prediction.as_ref())
        {
            style.color = [
                pred.text_color[0],
                pred.text_color[1],
                pred.text_color[2],
                255,
            ];
        }
        if style.font_families.is_empty()
            && let Some(pred) = blocks
                .iter()
                .find_map(|block| block.font_prediction.as_ref())
            && let Some(top) = pred.named_fonts.first()
        {
            style.font_families.push(top.name.clone());
            if let Some(sub) = self
                .google_fonts
                .substitute_font(top.serif, top.language.as_deref())
            {
                if !style.font_families.contains(&sub.to_string()) {
                    style.font_families.push(sub.to_string());
                }
            }
        }
        if style.font_families.is_empty()
            && let Some(font) = opts.document_font.as_ref()
        {
            style.font_families.push(font.clone());
        }
        if let Some(block) = blocks
            .iter()
            .find(|block| !block.translation.trim().is_empty())
        {
            for fb in font_families_for_text(block.translation.trim()) {
                if !style.font_families.contains(&fb) {
                    style.font_families.push(fb);
                }
            }
        }
        style
    }

    /// Resolve a set of font family candidates into a single PostScript name.
    pub fn resolve_post_script_name(
        &self,
        style: &TextStyle,
        text: Option<&str>,
    ) -> Result<String> {
        let fontbook = self
            .fontbook
            .lock()
            .map_err(|_| anyhow::anyhow!("failed to lock fontbook"))?;
        let faces = fontbook.all_families();

        let mut families = style.font_families.clone();
        if families.is_empty()
            && let Some(text) = text
        {
            tracing::debug!(
                "Families empty, applying script-based default font families for text: {}",
                text
            );
            apply_default_font_families(&mut families, text);
        }
        if families.is_empty() {
            families.push("ArialMT".to_string());
        }

        for candidate in &families {
            tracing::debug!("Attempting to resolve font candidate: {}", candidate);
            // 1. Exact PS name
            if let Some(face) = faces.iter().find(|f| f.post_script_name == *candidate) {
                tracing::debug!("Resolved via exact PS name: {}", face.post_script_name);
                return Ok(face.post_script_name.clone());
            }

            // 2. Google Font variant
            let (family, weight, style_str) = crate::google_fonts::parse_variant_query(candidate);
            if candidate.contains(':')
                && self
                    .google_fonts
                    .read_cached_variant(family, weight, style_str)
                    .map(|opt| opt.is_some())
                    .unwrap_or(false)
            {
                tracing::debug!("Resolved via Google Font variant: {}", candidate);
                return Ok(candidate.clone());
            }

            // 3. Fuzzy family name
            if let Some(psn) = face_post_script_name(&faces, candidate) {
                tracing::debug!("Resolved via fuzzy family name: {}", psn);
                return Ok(psn);
            }

            // 4. Base Google Font
            if self
                .google_fonts
                .read_cached_file(candidate)
                .map(|opt| opt.is_some())
                .unwrap_or(false)
            {
                tracing::debug!("Resolved via base Google Font: {}", candidate);
                return Ok(candidate.clone());
            }
        }

        tracing::warn!(?families, "font resolution failed, falling back to ArialMT");
        Ok("ArialMT".to_string())
    }

    fn select_font(&self, style: &TextStyle) -> Result<Font> {
        let mut fontbook = self
            .fontbook
            .lock()
            .map_err(|_| anyhow::anyhow!("failed to lock fontbook"))?;
        for candidate in &style.font_families {
            let faces = fontbook.all_families();

            // 1. Try exact PostScript name match first (most reliable for variants)
            if let Some(face) = faces.iter().find(|f| f.post_script_name == *candidate) {
                return fontbook.load_font(face.id);
            }

            // 2. Check if it's a Google Font variant (Family:WeightStyle)
            let (family, weight, style_str) = crate::google_fonts::parse_variant_query(candidate);
            if candidate.contains(':')
                && let Some(data) = self
                    .google_fonts
                    .read_cached_variant(family, weight, style_str)?
            {
                let mut font = fontbook.load_from_bytes(data)?;

                // Explicitly set the weight and style for variable font instancing
                font.weight = weight;
                font.style = style_str.to_string();

                return Ok(font);
            }

            // 3. Try fuzzy family name match
            if let Some(psn) = face_post_script_name(&faces, candidate) {
                return fontbook.query(&psn);
            }

            // 4. Try base Google Font file
            if let Some(data) = self.google_fonts.read_cached_file(candidate)? {
                return fontbook.load_from_bytes(data);
            }
        }
        Err(anyhow::anyhow!(
            "no font found for candidates: {:?}",
            style.font_families
        ))
    }
}

// ---------------------------------------------------------------------------
// Helpers: font sizing
// ---------------------------------------------------------------------------

#[cfg(test)]
const MASK_COLLISION_ALPHA_THRESHOLD: u8 = 8;
const FIT_EPSILON: f32 = 0.5;

struct RenderedTextCandidate {
    image: RgbaImage,
    transform: Transform,
}

struct RenderFitCandidate {
    image: RgbaImage,
    transform: Transform,
    font_size: f32,
    fits: bool,
}

struct ClippedSprite {
    image: DynamicImage,
    transform: Transform,
    was_clipped: bool,
}

#[cfg(test)]
struct FittedLayout<'a> {
    layout: LayoutRun<'a>,
    fits: bool,
}

#[cfg(test)]
struct MaskCollisionAttempt {
    candidate: RenderedTextCandidate,
    valid: bool,
}

fn min_font_size_for_image(image_width: u32, image_height: u32) -> f32 {
    let max_dim = image_width.max(image_height) as f32;
    (max_dim / 60.0).clamp(14.0, 36.0)
}

/// Minimum font size for a given layout box, used per-block as a higher floor
/// so short text in a large bubble doesn't stay tiny.
fn min_font_size_for_box(layout_box: LayoutBox, global_min: f32) -> f32 {
    let by_box = layout_box.height.max(layout_box.width) / 5.0;
    by_box.max(global_min)
}

/// Maximum font size for the given layout box, derived from its dimensions.
/// Caps extreme cases (huge empty bubble + short text → giant glyphs).
fn max_font_size_for_box(layout_box: LayoutBox, min_size: f32) -> f32 {
    const GLOBAL_CAP_PX: f32 = 120.0;
    let by_height = layout_box.height * 0.55;
    let by_width = layout_box.width * 0.9;
    by_height.min(by_width).clamp(min_size + 1.0, GLOBAL_CAP_PX)
}

fn font_size_hint(block: &RenderBlockInput) -> Option<f32> {
    block
        .style
        .as_ref()
        .and_then(|style| style.font_size)
        .or_else(|| block.font_prediction.as_ref().map(|p| p.font_size_px))
        .filter(|size| size.is_finite() && *size > 0.0)
}

fn resolve_page_line_height(blocks: &[RenderBlockInput]) -> f32 {
    blocks
        .iter()
        .filter_map(|block| block.font_prediction.as_ref().map(|p| p.line_height))
        .find(|scale| scale.is_finite() && *scale > 0.0)
        .map(|scale| scale.clamp(0.8, 2.0))
        .unwrap_or(1.3)
}

/// Binary-search the largest integer font size whose shaped layout fits inside
/// the constraint box. User/predicted font sizes are upper-bound hints, not
/// overflow-permitting overrides.
#[cfg(test)]
fn fit_font_size<'a>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    constraint_width: f32,
    constraint_height: f32,
    font_size_hint: Option<f32>,
    min_size: f32,
    max_size: f32,
) -> Result<FittedLayout<'a>> {
    let run_at = |size: f32| -> Result<LayoutRun<'a>> {
        layout_builder
            .clone()
            .with_font_size(size.max(1.0))
            .with_max_width(constraint_width)
            .with_max_height(constraint_height)
            .run(text)
    };

    let fits =
        |run: &LayoutRun<'a>| run.width <= constraint_width && run.height <= constraint_height;

    const ABSOLUTE_MIN: i32 = 4;
    let soft_min = min_size.max(1.0).round() as i32;
    let max_size = (max_size.round() as i32).max(soft_min);
    let max_size = font_size_hint
        .filter(|size| size.is_finite() && *size > 0.0)
        .map(|size| (size.round() as i32).clamp(ABSOLUTE_MIN, max_size))
        .unwrap_or(max_size);
    let effective_min = ABSOLUTE_MIN.min(soft_min).min(max_size);

    let at_max = run_at(max_size as f32)?;
    if fits(&at_max) {
        return Ok(FittedLayout {
            layout: at_max,
            fits: true,
        });
    }

    let mut lo = effective_min;
    let mut hi = max_size - 1;
    let mut best = run_at(effective_min as f32)?;
    if !fits(&best) {
        return Ok(FittedLayout {
            layout: best,
            fits: false,
        });
    }
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let candidate = run_at(mid as f32)?;
        if fits(&candidate) {
            best = candidate;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    Ok(FittedLayout {
        layout: best,
        fits: true,
    })
}

#[allow(clippy::too_many_arguments)]
fn fit_rendered_text<'a, F>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    layout_box: LayoutBox,
    font_size_hint: Option<f32>,
    min_size: f32,
    max_size: f32,
    render_candidate: &mut F,
) -> Result<RenderFitCandidate>
where
    F: FnMut(&LayoutRun<'a>) -> Result<RenderedTextCandidate>,
{
    let mut run_at = |size: f32| -> Result<RenderFitCandidate> {
        let layout = layout_builder
            .clone()
            .with_font_size(size.max(1.0))
            .with_max_width(layout_box.width)
            .with_max_height(layout_box.height)
            .run(text)?;
        let layout_fits = layout.width <= layout_box.width + FIT_EPSILON
            && layout.height <= layout_box.height + FIT_EPSILON;
        let font_size = layout.font_size;
        let candidate = render_candidate(&layout)?;
        let rendered_fits = candidate.image.width() as f32 <= layout_box.width + FIT_EPSILON
            && candidate.image.height() as f32 <= layout_box.height + FIT_EPSILON;
        Ok(RenderFitCandidate {
            image: candidate.image,
            transform: candidate.transform,
            font_size,
            fits: layout_fits && rendered_fits,
        })
    };

    const ABSOLUTE_MIN: i32 = 4;
    let soft_min = min_size.max(1.0).round() as i32;
    let max_size = (max_size.round() as i32).max(soft_min);
    let max_size = font_size_hint
        .filter(|size| size.is_finite() && *size > 0.0)
        .map(|size| (size.round() as i32).clamp(ABSOLUTE_MIN, max_size))
        .unwrap_or(max_size);
    let effective_min = ABSOLUTE_MIN.min(soft_min).min(max_size);

    let at_max = run_at(max_size as f32)?;
    if at_max.fits {
        return Ok(at_max);
    }

    let mut lo = effective_min;
    let mut hi = max_size - 1;
    let mut best = run_at(effective_min as f32)?;
    if !best.fits {
        return Ok(best);
    }
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let candidate = run_at(mid as f32)?;
        if candidate.fits {
            best = candidate;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    Ok(best)
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn fit_rendered_with_mask_collision<'a, F>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    layout_box: LayoutBox,
    explicit_size: Option<f32>,
    preferred_size: Option<f32>,
    min_size: f32,
    max_size: f32,
    mask: &GrayImage,
    bubble_id: u8,
    render_candidate: &mut F,
) -> Result<RenderedTextCandidate>
where
    F: FnMut(&LayoutRun<'a>) -> Result<RenderedTextCandidate>,
{
    if let Some(size) = explicit_size {
        let attempt = render_mask_collision_attempt(
            layout_builder,
            text,
            layout_box,
            size.max(1.0),
            mask,
            bubble_id,
            render_candidate,
        )?;
        return Ok(attempt.candidate);
    }

    const ABSOLUTE_MIN: i32 = 4;
    let soft_min = min_size.max(1.0).round() as i32;
    let max_size = (max_size.max(1.0).round() as i32).max(soft_min);
    let max_size = preferred_size
        .filter(|size| size.is_finite() && *size > 0.0)
        .map(|size| (size.round() as i32).clamp(ABSOLUTE_MIN, max_size))
        .unwrap_or(max_size);
    let effective_min = ABSOLUTE_MIN.min(soft_min).min(max_size);

    if let Some(candidate) = try_mask_collision_size(
        layout_builder,
        text,
        layout_box,
        max_size as f32,
        mask,
        bubble_id,
        render_candidate,
    )? {
        return Ok(candidate);
    }

    let min_attempt = render_mask_collision_attempt(
        layout_builder,
        text,
        layout_box,
        effective_min as f32,
        mask,
        bubble_id,
        render_candidate,
    )?;
    if !min_attempt.valid {
        return Ok(min_attempt.candidate);
    }
    let mut best = min_attempt.candidate;

    let mut lo = effective_min + 1;
    let mut hi = max_size - 1;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        if let Some(candidate) = try_mask_collision_size(
            layout_builder,
            text,
            layout_box,
            mid as f32,
            mask,
            bubble_id,
            render_candidate,
        )? {
            best = candidate;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }

    Ok(best)
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn try_mask_collision_size<'a, F>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    layout_box: LayoutBox,
    font_size: f32,
    mask: &GrayImage,
    bubble_id: u8,
    render_candidate: &mut F,
) -> Result<Option<RenderedTextCandidate>>
where
    F: FnMut(&LayoutRun<'a>) -> Result<RenderedTextCandidate>,
{
    let layout = run_collision_layout_at(layout_builder, text, layout_box, font_size)?;
    let fits_layout_box = layout_fits_collision_attempt(&layout, layout_box);
    if !fits_layout_box {
        return Ok(None);
    }

    let candidate = render_candidate(&layout)?;
    if sprite_collides_with_bubble_mask(&candidate.image, &candidate.transform, mask, bubble_id) {
        return Ok(None);
    }
    Ok(Some(candidate))
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn render_mask_collision_attempt<'a, F>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    layout_box: LayoutBox,
    font_size: f32,
    mask: &GrayImage,
    bubble_id: u8,
    render_candidate: &mut F,
) -> Result<MaskCollisionAttempt>
where
    F: FnMut(&LayoutRun<'a>) -> Result<RenderedTextCandidate>,
{
    let layout = run_collision_layout_at(layout_builder, text, layout_box, font_size)?;
    let fits_layout_box = layout_fits_collision_attempt(&layout, layout_box);
    let candidate = render_candidate(&layout)?;
    let valid = fits_layout_box
        && !sprite_collides_with_bubble_mask(
            &candidate.image,
            &candidate.transform,
            mask,
            bubble_id,
        );
    Ok(MaskCollisionAttempt { candidate, valid })
}

#[cfg(test)]
fn run_collision_layout_at<'a>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    layout_box: LayoutBox,
    font_size: f32,
) -> Result<LayoutRun<'a>> {
    layout_builder
        .clone()
        .with_font_size(font_size.max(1.0))
        .with_max_width(layout_box.width.max(1.0))
        .with_max_height(layout_box.height.max(1.0))
        .run(text)
}

#[cfg(test)]
fn layout_fits_collision_attempt(layout: &LayoutRun<'_>, layout_box: LayoutBox) -> bool {
    layout.width <= layout_box.width + FIT_EPSILON
        && layout.height <= layout_box.height + FIT_EPSILON
}

#[cfg(test)]
fn sprite_collides_with_bubble_mask(
    sprite: &RgbaImage,
    transform: &Transform,
    mask: &GrayImage,
    bubble_id: u8,
) -> bool {
    let origin_x = transform.x.round() as i32;
    let origin_y = transform.y.round() as i32;
    let mask_w = mask.width() as i32;
    let mask_h = mask.height() as i32;

    for (x, y, pixel) in sprite.enumerate_pixels() {
        if pixel.0[3] <= MASK_COLLISION_ALPHA_THRESHOLD {
            continue;
        }
        let mask_x = origin_x + x as i32;
        let mask_y = origin_y + y as i32;
        if mask_x < 0 || mask_y < 0 || mask_x >= mask_w || mask_y >= mask_h {
            return true;
        }
        if mask.get_pixel(mask_x as u32, mask_y as u32).0[0] != bubble_id {
            return true;
        }
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg(test)]
struct ResolvedLayoutBox {
    seed_box: LayoutBox,
    layout_box: LayoutBox,
    bubble_id: Option<u8>,
}

#[cfg(test)]
fn resolve_layout_boxes(
    blocks: &[RenderBlockInput],
    bubble_index: Option<&BubbleIndex>,
) -> Vec<ResolvedLayoutBox> {
    let Some(bubble_index) = bubble_index else {
        return blocks
            .iter()
            .map(|block| {
                let seed_box = seed_layout_box(block);
                ResolvedLayoutBox {
                    seed_box,
                    layout_box: seed_box,
                    bubble_id: None,
                }
            })
            .collect();
    };

    let mut counts: HashMap<u8, usize> = HashMap::new();
    let mut matches = Vec::with_capacity(blocks.len());

    for block in blocks {
        let seed_box = seed_layout_box(block);
        let translation = block.translation.trim();
        let bubble_match = if block.lock_layout_box || translation.is_empty() {
            None
        } else {
            let layout_source = layout_source_from_input(block, translation);
            let writing_mode = writing_mode_for_block(&layout_source);
            bubble_index.lookup_match(seed_box, writing_mode)
        };
        if let Some(matched) = bubble_match {
            *counts.entry(matched.id).or_insert(0) += 1;
        }
        matches.push((seed_box, bubble_match));
    }

    matches
        .into_iter()
        .map(|(seed_box, bubble_match)| match bubble_match {
            Some(matched) => {
                // Expand into the bubble's safe area even when multiple
                // blocks share one bubble — the collision check in
                // render_one prevents overlapping sprite output.
                ResolvedLayoutBox {
                    seed_box,
                    layout_box: matched.layout_box,
                    bubble_id: Some(matched.id),
                }
            }
            None => ResolvedLayoutBox {
                seed_box,
                layout_box: seed_box,
                bubble_id: None,
            },
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers: font families, fallbacks
// ---------------------------------------------------------------------------

fn apply_default_font_families(font_families: &mut Vec<String>, text: &str) {
    if font_families.is_empty() {
        *font_families = font_families_for_text(text);
    }
}

fn load_symbol_fallbacks(fontbook: &mut FontBook) -> Vec<Font> {
    let candidates = [
        "Segoe UI Symbol",
        "Segoe UI Emoji",
        "Noto Sans Symbols",
        "Noto Sans Symbols2",
        "Noto Color Emoji",
        "Apple Color Emoji",
        "Apple Symbols",
        "Symbola",
        "Arial Unicode MS",
    ];
    let faces = fontbook.all_families();
    candidates
        .iter()
        .filter_map(|candidate| face_post_script_name(&faces, candidate))
        .filter_map(|post_script_name| fontbook.query(&post_script_name).ok())
        .collect()
}

fn face_post_script_name(faces: &[FaceInfo], candidate: &str) -> Option<String> {
    let candidate_lower = candidate.trim().to_lowercase();
    faces
        .iter()
        .find(|face| {
            face.post_script_name.to_lowercase() == candidate_lower
                || face
                    .families
                    .iter()
                    .any(|(family, _)| family.to_lowercase() == candidate_lower)
        })
        .map(|face| face.post_script_name.clone())
        .filter(|post_script_name| !post_script_name.is_empty())
}

fn layout_source_from_input(block: &RenderBlockInput, translation: &str) -> RenderBlock {
    RenderBlock {
        x: block.transform.x,
        y: block.transform.y,
        width: block.transform.width.max(1.0),
        height: block.transform.height.max(1.0),
        text: translation.to_string(),
        source_direction: block.source_direction.map(core_direction_to_renderer),
    }
}

fn seed_layout_box(block: &RenderBlockInput) -> LayoutBox {
    LayoutBox {
        x: block.transform.x,
        y: block.transform.y,
        width: block.transform.width.max(1.0),
        height: block.transform.height.max(1.0),
    }
}

// ---------------------------------------------------------------------------
// Helpers: stroke resolution
// ---------------------------------------------------------------------------

fn default_stroke_width(font_size: f32) -> f32 {
    (font_size * 0.10).clamp(1.2, 8.0)
}

fn contrasting_stroke_color(text_color: [u8; 4]) -> [u8; 4] {
    let luminance =
        0.299 * text_color[0] as f32 + 0.587 * text_color[1] as f32 + 0.114 * text_color[2] as f32;
    if luminance > 128.0 {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
    }
}

fn resolve_stroke_style(
    font_prediction: Option<&FontPrediction>,
    block_stroke: Option<&TextStrokeStyle>,
    global_stroke: Option<&TextStrokeStyle>,
    font_size: f32,
    text_color: [u8; 4],
) -> Option<RenderStrokeOptions> {
    if let Some(stroke) = block_stroke {
        if !stroke.enabled {
            return None;
        }
        return Some(RenderStrokeOptions {
            color: stroke.color,
            width_px: stroke
                .width_px
                .unwrap_or_else(|| default_stroke_width(font_size)),
        });
    }
    if let Some(stroke) = global_stroke {
        if !stroke.enabled {
            return None;
        }
        return Some(RenderStrokeOptions {
            color: stroke.color,
            width_px: stroke
                .width_px
                .unwrap_or_else(|| default_stroke_width(font_size)),
        });
    }
    let auto_stroke_color = contrasting_stroke_color(text_color);
    if let Some(pred) = font_prediction
        && pred.stroke_width_px > 0.0
    {
        return Some(RenderStrokeOptions {
            color: auto_stroke_color,
            width_px: pred.stroke_width_px,
        });
    }
    Some(RenderStrokeOptions {
        color: auto_stroke_color,
        width_px: default_stroke_width(font_size),
    })
}

#[cfg(test)]
fn resolve_text_color(
    explicit_style: Option<&TextStyle>,
    _derived_style: &TextStyle,
    font_prediction: Option<&FontPrediction>,
) -> [u8; 4] {
    if let Some(pred) = font_prediction {
        let predicted = [
            pred.text_color[0],
            pred.text_color[1],
            pred.text_color[2],
            255,
        ];
        // If the only style color we have is the implicit default black, prefer
        // the detector's predicted color — that's the source image's actual text
        // colour, not a user override.
        match explicit_style {
            Some(s) if s.color != [0, 0, 0, 255] => return s.color,
            _ => return predicted,
        }
    }
    match explicit_style {
        Some(s) => s.color,
        None => [0, 0, 0, 255],
    }
}

// ---------------------------------------------------------------------------
// Helpers: type conversions
// ---------------------------------------------------------------------------

fn shader_core_to_renderer(e: TextShaderEffect) -> RendererEffect {
    RendererEffect {
        italic: e.italic,
        bold: e.bold,
    }
}

fn core_align_to_renderer(a: koharu_core::TextAlign) -> RendererTextAlign {
    match a {
        koharu_core::TextAlign::Left => RendererTextAlign::Left,
        koharu_core::TextAlign::Center => RendererTextAlign::Center,
        koharu_core::TextAlign::Right => RendererTextAlign::Right,
    }
}

fn core_direction_to_renderer(d: TextDirection) -> RendererTextDirection {
    match d {
        TextDirection::Horizontal => RendererTextDirection::Horizontal,
        TextDirection::Vertical => RendererTextDirection::Vertical,
    }
}

fn rendered_direction_for_writing_mode(writing_mode: WritingMode) -> TextDirection {
    match writing_mode {
        WritingMode::Horizontal => TextDirection::Horizontal,
        WritingMode::VerticalRl => TextDirection::Vertical,
    }
}

/// Clip sprite to fit inside the layout box so overflow never spills out.
/// The returned transform matches the clipped sprite placement/dimensions.
fn clip_to_box(sprite: DynamicImage, box_: LayoutBox, transform: &Transform) -> ClippedSprite {
    let bx = box_.x;
    let by = box_.y;
    let bw = box_.width.max(1.0);
    let bh = box_.height.max(1.0);

    // Sprite is placed at (transform.x, transform.y) in page coords.
    let overlap_x0 = transform.x.max(bx);
    let overlap_y0 = transform.y.max(by);
    let overlap_x1 = (transform.x + transform.width).min(bx + bw);
    let overlap_y1 = (transform.y + transform.height).min(by + bh);

    if overlap_x0 >= overlap_x1 || overlap_y0 >= overlap_y1 {
        let empty = Transform {
            x: overlap_x0,
            y: overlap_y0,
            width: 1.0,
            height: 1.0,
            rotation_deg: transform.rotation_deg,
        };
        return ClippedSprite {
            image: DynamicImage::ImageRgba8(RgbaImage::new(1, 1)),
            transform: empty,
            was_clipped: true,
        };
    }

    let local_x = (overlap_x0 - transform.x) as u32;
    let local_y = (overlap_y0 - transform.y) as u32;
    let crop_w = (overlap_x1 - overlap_x0) as u32;
    let crop_h = (overlap_y1 - overlap_y0) as u32;

    let cropped = sprite.crop_imm(local_x, local_y, crop_w, crop_h);
    let cropped_transform = Transform {
        x: overlap_x0,
        y: overlap_y0,
        width: crop_w as f32,
        height: crop_h as f32,
        rotation_deg: transform.rotation_deg,
    };
    ClippedSprite {
        image: DynamicImage::ImageRgba8(cropped.to_rgba8()),
        transform: cropped_transform,
        was_clipped: local_x != 0
            || local_y != 0
            || crop_w != sprite.width()
            || crop_h != sprite.height(),
    }
}

// ---------------------------------------------------------------------------
// Helpers: placement
// ---------------------------------------------------------------------------

fn centred_sprite_transform(
    anchor_box: LayoutBox,
    sprite_width: u32,
    sprite_height: u32,
    rotation_deg: f32,
) -> Transform {
    let sprite_w = sprite_width as f32;
    let sprite_h = sprite_height as f32;
    let cx = anchor_box.x + anchor_box.width * 0.5;
    let cy = anchor_box.y + anchor_box.height * 0.5;
    Transform {
        x: (cx - sprite_w * 0.5).round(),
        y: (cy - sprite_h * 0.5).round(),
        width: sprite_w,
        height: sprite_h,
        rotation_deg,
    }
}

fn find_input(blocks: &[RenderBlockInput], id: NodeId) -> &RenderBlockInput {
    blocks
        .iter()
        .find(|b| b.node_id == id)
        .expect("rendered_block must have matching input")
}

fn placement_origin(input: &RenderBlockInput, expanded: &Option<Transform>) -> (f32, f32) {
    if let Some(t) = expanded {
        (t.x.round(), t.y.round())
    } else {
        (input.transform.x, input.transform.y)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma, Rgba, RgbaImage};
    use koharu_core::NodeId;

    #[test]
    fn default_font_families_should_fill_empty_list() {
        let mut font_families = Vec::new();
        apply_default_font_families(&mut font_families, "hello");
        assert!(!font_families.is_empty());
    }

    #[test]
    fn default_stroke_color_uses_black_for_light_text() {
        let stroke = resolve_stroke_style(None, None, None, 16.0, [255, 255, 255, 255])
            .expect("default stroke should be present");
        assert_eq!(stroke.color, [0, 0, 0, 255]);
        assert_eq!(stroke.width_px, 1.6);
    }

    #[test]
    fn predicted_stroke_width_keeps_auto_black_or_white_color() {
        let prediction = FontPrediction {
            stroke_color: [12, 34, 56],
            stroke_width_px: 3.0,
            ..Default::default()
        };
        let stroke =
            resolve_stroke_style(Some(&prediction), None, None, 18.0, [255, 255, 255, 255])
                .expect("predicted stroke should be present");
        assert_eq!(stroke.color, [0, 0, 0, 255]);
        assert_eq!(stroke.width_px, 3.0);
    }

    #[test]
    fn explicit_block_stroke_color_is_preserved_even_if_it_matches_text() {
        let stroke = resolve_stroke_style(
            None,
            Some(&TextStrokeStyle {
                enabled: true,
                color: [255, 255, 255, 255],
                width_px: Some(2.0),
            }),
            None,
            18.0,
            [255, 255, 255, 255],
        )
        .expect("explicit stroke should be present");
        assert_eq!(stroke.color, [255, 255, 255, 255]);
        assert_eq!(stroke.width_px, 2.0);
    }

    #[test]
    fn predicted_text_color_wins_without_explicit_style() {
        let derived = TextStyle {
            font_families: Vec::new(),
            font_size: None,
            color: [0, 0, 0, 255],
            effect: None,
            stroke: None,
            text_align: None,
        };
        let prediction = FontPrediction {
            text_color: [12, 34, 56],
            ..Default::default()
        };
        assert_eq!(
            resolve_text_color(None, &derived, Some(&prediction)),
            [12, 34, 56, 255]
        );
    }

    #[test]
    fn explicit_text_color_wins_over_prediction() {
        let explicit = TextStyle {
            font_families: Vec::new(),
            font_size: None,
            color: [200, 100, 50, 255],
            effect: None,
            stroke: None,
            text_align: None,
        };
        let prediction = FontPrediction {
            text_color: [12, 34, 56],
            ..Default::default()
        };
        assert_eq!(
            resolve_text_color(Some(&explicit), &explicit, Some(&prediction)),
            [200, 100, 50, 255]
        );
    }

    #[test]
    fn mask_collision_fit_renders_min_size_when_no_safe_size_exists() -> Result<()> {
        let font = any_system_font();
        let layout_builder = TextLayout::new(&font, None);
        let layout_box = LayoutBox {
            x: 0.0,
            y: 0.0,
            width: 24.0,
            height: 12.0,
        };
        let mask = GrayImage::from_pixel(64, 64, Luma([0u8]));
        let mut rendered_sizes = Vec::new();
        let mut render_candidate = |layout: &LayoutRun<'_>| -> Result<RenderedTextCandidate> {
            rendered_sizes.push(layout.font_size);
            let width = layout.width.ceil().max(1.0) as u32;
            let height = layout.height.ceil().max(1.0) as u32;
            Ok(RenderedTextCandidate {
                image: RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 255])),
                transform: Transform {
                    x: 0.0,
                    y: 0.0,
                    width: width as f32,
                    height: height as f32,
                    rotation_deg: 0.0,
                },
            })
        };

        let candidate = fit_rendered_with_mask_collision(
            &layout_builder,
            "overflowing text",
            layout_box,
            None,
            None,
            12.0,
            18.0,
            &mask,
            1,
            &mut render_candidate,
        )?;

        assert_eq!(rendered_sizes.last().copied(), Some(4.0));
        assert!(candidate.image.width() >= 1);
        assert!(candidate.image.height() >= 1);
        Ok(())
    }

    #[test]
    fn fit_font_size_shrinks_predicted_size_to_fit_box() -> Result<()> {
        let font = any_system_font();
        let builder = TextLayout::new(&font, None);
        let fitted = fit_font_size(
            &builder,
            "This translation is intentionally too long for the requested size.",
            96.0,
            36.0,
            Some(48.0),
            12.0,
            80.0,
        )?;

        assert!(fitted.fits);
        assert!(fitted.layout.font_size < 48.0);
        assert!(fitted.layout.width <= 96.0 + FIT_EPSILON);
        assert!(fitted.layout.height <= 36.0 + FIT_EPSILON);
        Ok(())
    }

    #[test]
    fn fit_font_size_shrinks_user_size_hint_to_fit_box() -> Result<()> {
        let font = any_system_font();
        let builder = TextLayout::new(&font, None);
        let fitted = fit_font_size(
            &builder,
            "User requested font size should shrink when the box is tiny.",
            64.0,
            32.0,
            Some(42.0),
            4.0,
            80.0,
        )?;

        assert!(fitted.layout.font_size < 42.0);
        assert!(fitted.layout.width <= 64.0 + FIT_EPSILON);
        assert!(fitted.layout.height <= 32.0 + FIT_EPSILON);
        Ok(())
    }

    #[test]
    fn strict_render_page_ignores_bubble_expansion() -> Result<()> {
        let renderer = Renderer::new()?;
        let base =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255])));
        let mut bubble = GrayImage::from_pixel(200, 200, Luma([0u8]));
        paint_rect(&mut bubble, 10, 10, 190, 190, 1);
        let blocks = vec![block(70.0, 70.0, 32.0, 24.0, "hello world")];

        let output = renderer.render_page(
            &base,
            None,
            Some(&DynamicImage::ImageLuma8(bubble)),
            200,
            200,
            &blocks,
            &PageRenderOptions::default(),
        )?;
        let transform = output.blocks[0]
            .expanded_transform
            .expect("strict renderer should persist sprite transform");

        assert!(transform.x >= blocks[0].transform.x);
        assert!(transform.y >= blocks[0].transform.y);
        assert!(transform.x + transform.width <= blocks[0].transform.x + blocks[0].transform.width);
        assert!(
            transform.y + transform.height <= blocks[0].transform.y + blocks[0].transform.height
        );
        Ok(())
    }

    #[test]
    fn resolved_layout_boxes_helper_can_still_expand_bubbles() {
        let mut mask = GrayImage::from_pixel(200, 200, Luma([0u8]));
        paint_rect(&mut mask, 20, 20, 180, 180, 1);
        let index = BubbleIndex::new(mask);
        let blocks = vec![block(70.0, 70.0, 20.0, 30.0, "hello")];

        let layout_boxes = resolve_layout_boxes(&blocks, Some(&index));

        assert!(layout_boxes[0].layout_box.width > blocks[0].transform.width);
        assert!(layout_boxes[0].layout_box.height > blocks[0].transform.height);
        assert_eq!(layout_boxes[0].bubble_id, Some(1));
    }

    #[test]
    fn locked_block_keeps_manual_layout_box_inside_bubble() {
        let mut mask = GrayImage::from_pixel(200, 200, Luma([0u8]));
        paint_rect(&mut mask, 20, 20, 180, 180, 1);
        let index = BubbleIndex::new(mask);
        let mut locked = block(70.0, 70.0, 20.0, 30.0, "hello");
        locked.lock_layout_box = true;
        let blocks = vec![locked];

        let layout_boxes = resolve_layout_boxes(&blocks, Some(&index));

        assert_eq!(layout_boxes[0].layout_box, seed_layout_box(&blocks[0]));
        assert_eq!(layout_boxes[0].bubble_id, None);
    }

    #[test]
    fn page_style_template_reuses_non_size_style_across_blocks() -> Result<()> {
        let renderer = Renderer::new()?;
        let mut a = block(0.0, 0.0, 100.0, 40.0, "first");
        a.style = Some(TextStyle {
            font_families: vec!["Arial".to_string()],
            font_size: Some(48.0),
            color: [10, 20, 30, 255],
            effect: Some(TextShaderEffect {
                italic: false,
                bold: true,
            }),
            stroke: Some(TextStrokeStyle {
                enabled: true,
                color: [255, 255, 255, 255],
                width_px: Some(2.0),
            }),
            text_align: Some(koharu_core::TextAlign::Center),
        });
        let mut b = block(0.0, 50.0, 100.0, 40.0, "second");
        b.style = Some(TextStyle {
            font_families: vec!["Times New Roman".to_string()],
            font_size: Some(12.0),
            color: [200, 10, 10, 255],
            effect: None,
            stroke: None,
            text_align: Some(koharu_core::TextAlign::Right),
        });

        let style = renderer
            .resolve_page_style_template(&[a.clone(), b.clone()], &PageRenderOptions::default());

        assert_eq!(
            style.font_families.first().map(String::as_str),
            Some("Arial")
        );
        assert_eq!(style.font_size, None);
        assert_eq!(style.color, [10, 20, 30, 255]);
        assert!(style.effect.expect("effect").bold);
        assert_eq!(style.stroke.expect("stroke").width_px, Some(2.0));
        assert_eq!(font_size_hint(&a), Some(48.0));
        assert_eq!(font_size_hint(&b), Some(12.0));
        Ok(())
    }

    #[test]
    fn mask_collision_fit_shrinks_predicted_size() -> Result<()> {
        let font = any_system_font();
        let layout_builder = TextLayout::new(&font, None);
        let layout_box = LayoutBox {
            x: 0.0,
            y: 0.0,
            width: 80.0,
            height: 36.0,
        };
        let mask = GrayImage::from_pixel(160, 160, Luma([1u8]));
        let mut rendered_sizes = Vec::new();
        let mut render_candidate = |layout: &LayoutRun<'_>| -> Result<RenderedTextCandidate> {
            rendered_sizes.push(layout.font_size);
            let width = layout.width.ceil().max(1.0) as u32;
            let height = layout.height.ceil().max(1.0) as u32;
            Ok(RenderedTextCandidate {
                image: RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 255])),
                transform: Transform {
                    x: 0.0,
                    y: 0.0,
                    width: width as f32,
                    height: height as f32,
                    rotation_deg: 0.0,
                },
            })
        };

        let candidate = fit_rendered_with_mask_collision(
            &layout_builder,
            "This translation should shrink from the predicted font size.",
            layout_box,
            None,
            Some(48.0),
            8.0,
            80.0,
            &mask,
            1,
            &mut render_candidate,
        )?;

        assert!(rendered_sizes.iter().all(|size| *size <= 48.0));
        assert!(candidate.transform.width <= layout_box.width + FIT_EPSILON);
        assert!(candidate.transform.height <= layout_box.height + FIT_EPSILON);
        Ok(())
    }

    #[test]
    fn clip_to_box_adjusts_transform() {
        let sprite =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(200, 100, Rgba([0, 0, 0, 255])));
        let box_ = LayoutBox {
            x: 10.0,
            y: 10.0,
            width: 50.0,
            height: 50.0,
        };
        let transform = Transform {
            x: 5.0,
            y: 5.0,
            width: 200.0,
            height: 100.0,
            rotation_deg: 0.0,
        };

        let clipped = clip_to_box(sprite, box_, &transform);

        assert!(clipped.was_clipped);
        assert_eq!(clipped.image.width(), 50);
        assert_eq!(clipped.image.height(), 50);
        assert_eq!(clipped.transform.x, 10.0);
        assert_eq!(clipped.transform.y, 10.0);
        assert_eq!(clipped.transform.width, 50.0);
        assert_eq!(clipped.transform.height, 50.0);
    }

    #[test]
    fn clip_to_box_keeps_transform_when_inside_box() {
        let sprite = DynamicImage::ImageRgba8(RgbaImage::from_pixel(20, 10, Rgba([0, 0, 0, 255])));
        let box_ = LayoutBox {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        let transform = Transform {
            x: 15.0,
            y: 25.0,
            width: 20.0,
            height: 10.0,
            rotation_deg: 0.0,
        };

        let clipped = clip_to_box(sprite, box_, &transform);

        assert!(!clipped.was_clipped);
        assert_eq!(clipped.image.width(), 20);
        assert_eq!(clipped.image.height(), 10);
        assert_eq!(clipped.transform.x, transform.x);
        assert_eq!(clipped.transform.y, transform.y);
        assert_eq!(clipped.transform.width, transform.width);
        assert_eq!(clipped.transform.height, transform.height);
        assert_eq!(clipped.transform.rotation_deg, transform.rotation_deg);
    }

    #[test]
    fn mask_collision_detects_alpha_outside_matched_bubble() {
        let mut mask = GrayImage::from_pixel(10, 10, Luma([0u8]));
        paint_rect(&mut mask, 2, 2, 8, 8, 1);
        let sprite = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));

        let inside = Transform {
            x: 3.0,
            y: 3.0,
            width: 4.0,
            height: 4.0,
            rotation_deg: 0.0,
        };
        assert!(!sprite_collides_with_bubble_mask(
            &sprite, &inside, &mask, 1
        ));

        let outside = Transform {
            x: 0.0,
            y: 0.0,
            width: 4.0,
            height: 4.0,
            rotation_deg: 0.0,
        };
        assert!(sprite_collides_with_bubble_mask(
            &sprite, &outside, &mask, 1
        ));
    }

    #[test]
    fn mask_collision_ignores_transparent_sprite_pixels() {
        let mask = GrayImage::from_pixel(4, 4, Luma([0u8]));
        let sprite = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 0]));
        let transform = Transform {
            x: 0.0,
            y: 0.0,
            width: 4.0,
            height: 4.0,
            rotation_deg: 0.0,
        };

        assert!(!sprite_collides_with_bubble_mask(
            &sprite, &transform, &mask, 1
        ));
    }

    fn block(x: f32, y: f32, width: f32, height: f32, translation: &str) -> RenderBlockInput {
        RenderBlockInput {
            node_id: NodeId::new(),
            transform: Transform {
                x,
                y,
                width,
                height,
                rotation_deg: 0.0,
            },
            translation: translation.to_string(),
            style: None,
            font_prediction: None,
            source_direction: None,
            rendered_direction: None,
            lock_layout_box: false,
        }
    }

    fn paint_rect(img: &mut GrayImage, x0: u32, y0: u32, x1: u32, y1: u32, value: u8) {
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, Luma([value]));
            }
        }
    }

    fn any_system_font() -> Font {
        let mut book = FontBook::new();
        let preferred = [
            "Yu Gothic",
            "MS Gothic",
            "Noto Sans CJK JP",
            "Noto Sans",
            "Arial",
            "DejaVu Sans",
            "Liberation Sans",
        ];

        for name in preferred {
            if let Some(post_script_name) = book
                .all_families()
                .into_iter()
                .find(|face| {
                    face.post_script_name == name
                        || face
                            .families
                            .iter()
                            .any(|(family, _)| family.as_str() == name)
                })
                .map(|face| face.post_script_name)
                .filter(|post_script_name| !post_script_name.is_empty())
                && let Ok(font) = book.query(&post_script_name)
            {
                return font;
            }
        }

        if let Some(face) = book
            .all_families()
            .into_iter()
            .find(|face| !face.post_script_name.is_empty())
        {
            return book
                .query(&face.post_script_name)
                .expect("failed to load first system font");
        }

        panic!("no system font available for tests");
    }

    #[test]
    fn centred_sprite_transform_anchors_to_provided_box_center() {
        let anchor = LayoutBox {
            x: 100.0,
            y: 100.0,
            width: 200.0,
            height: 100.0,
        };
        let sprite_w = 100;
        let sprite_h = 50;

        let transform = centred_sprite_transform(anchor, sprite_w, sprite_h, 0.0);

        // Center of anchor is (200, 150).
        // Sprite (100x50) centered on (200, 150) starts at (150, 125).
        assert_eq!(transform.x, 150.0);
        assert_eq!(transform.y, 125.0);
    }
}
