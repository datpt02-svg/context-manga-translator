//! Smart Fallback: after base OCR, runs OCR quality checks on every text node
//! and sends only uncertain/suspicious boxes to the Unlimited-OCR service.
//!
//! This module is called from the pipeline driver after the OCR engine step
//! when `UnlimitedOcrMode::SmartFallback` is active.

use anyhow::{Context, Result};
use base64::Engine as _;
use image::DynamicImage;
use koharu_core::{
    NodeDataPatch, NodeId, NodeKind, NodePatch, Op, Page, PageId, Scene, TextData, TextDataPatch,
    TextDirection, TextTranslationContext,
};
use koharu_ml::comic_text_detector::crop_text_block_bbox;

use crate::blobs::BlobStore;
use crate::pipeline::ocr_quality::{OcrQualityInput, assess_ocr_quality};
use crate::pipeline::unlimited_ocr_client::{
    UnlimitedCropImage, UnlimitedCropRequest, UnlimitedOcrClient,
};

/// Context struct mirroring the parts of `EngineCtx` that the fallback needs,
/// kept separate so it can be called outside the engine framework.
pub struct FallbackCtx<'a> {
    pub scene: &'a Scene,
    pub page: PageId,
    pub blobs: &'a BlobStore,
    pub service_url: &'a str,
}

/// Run the Smart Fallback logic for one page.
///
/// 1. Scan text nodes and select uncertain / suspicious ones.
/// 2. Crop selected boxes from the source image.
/// 3. Send one batch request to the Unlimited-OCR service.
/// 4. Return ops that update only the selected nodes.
///
/// If the service is unreachable, log a warning and mark the selected
/// nodes as `ocr_uncertain = true` without erasing base OCR text.
pub async fn apply_unlimited_ocr_fallback(ctx: FallbackCtx<'_>) -> Result<Vec<Op>> {
    let ctx_page = ctx.page;

    // 1. Select suspicious nodes
    let Some(page_ref) = ctx.scene.page(ctx.page) else {
        return Ok(Vec::new());
    };

    let selected = select_suspicious_nodes(page_ref);
    if selected.is_empty() {
        tracing::debug!("SmartFallback: no suspicious boxes on page {}", ctx.page);
        return Ok(Vec::new());
    }

    tracing::info!(
        "SmartFallback selected {} of {} boxes on page {}",
        selected.len(),
        page_ref
            .nodes
            .values()
            .filter(|n| matches!(n.kind, NodeKind::Text(_)))
            .count(),
        ctx.page,
    );

    // 2. Load source image and crop
    let image = match load_source_image(ctx.scene, ctx.page, ctx.blobs) {
        Ok(img) => img,
        Err(e) => {
            tracing::warn!("SmartFallback: failed to load source image: {e:#}");
            return Ok(mark_uncertain_ops(ctx.page, &selected));
        }
    };

    // 3. Build crops & batch request
    let mut crop_images = Vec::with_capacity(selected.len());
    for (node_id, tf, td) in &selected {
        let region = crate::pipeline::engines::support::text_node_to_region(tf, td);
        let crop = crop_text_block_bbox(&image, &region);
        let base64 = encode_png(&crop);
        crop_images.push(UnlimitedCropImage {
            id: node_id.to_string(),
            image_base64: base64,
        });
    }

    let client = UnlimitedOcrClient::new(ctx.service_url);
    let request = UnlimitedCropRequest {
        images: crop_images,
        language_hint: None,
        return_context: true,
    };

    // 4. Send batch
    let response = match client
        .ocr_crops(request)
        .await
        .with_context(|| format!("Unlimited-OCR fallback (service at {})", ctx.service_url))
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("SmartFallback service call failed, keeping base OCR: {e:#}");
            return Ok(mark_uncertain_ops(ctx.page, &selected));
        }
    };

    tracing::info!(
        "SmartFallback succeeded for {} of {} boxes",
        response.items.len(),
        selected.len(),
    );

    // 5. Build ops — map response items by id
    let mut item_map: std::collections::HashMap<
        String,
        &crate::pipeline::unlimited_ocr_client::UnlimitedOcrItem,
    > = std::collections::HashMap::new();
    for item in &response.items {
        item_map.insert(item.id.clone(), item);
    }

    let mut ops = Vec::with_capacity(selected.len());
    for (node_id, _tf, td) in &selected {
        let id_str = node_id.to_string();
        let Some(item) = item_map.get(&id_str) else {
            // Missing item — mark uncertain, keep base text
            ops.push(Op::UpdateNode {
                page: ctx_page,
                id: *node_id,
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(TextDataPatch {
                        ocr_uncertain: Some(true),
                        ..Default::default()
                    })),
                    transform: None,
                    visible: None,
                },
                prev: NodePatch::default(),
            });
            continue;
        };

        // Re-check quality on the returned text
        let quality_after = assess_ocr_quality(OcrQualityInput {
            text: Some(&item.text),
            detector_confidence: td.confidence,
            ocr_confidence: item.confidence,
            bbox_width: _tf.width,
            bbox_height: _tf.height,
            is_vertical: matches!(td.source_direction, Some(TextDirection::Vertical)),
        });

        let ctx_patch = map_translation_context(item);
        ops.push(Op::UpdateNode {
            page: ctx_page,
            id: *node_id,
            patch: NodePatch {
                data: Some(NodeDataPatch::Text(TextDataPatch {
                    text: Some(Some(item.text.clone())),
                    ocr_engine: Some(Some("unlimited-ocr".to_string())),
                    ocr_confidence: Some(item.confidence),
                    ocr_uncertain: Some(item.uncertain || quality_after.uncertain),
                    translation_context: ctx_patch,
                    ..Default::default()
                })),
                transform: None,
                visible: None,
            },
            prev: NodePatch::default(),
        });
    }

    Ok(ops)
}

/// Select text nodes that should be re-OCR'd.
/// A node is suspicious if:
/// - `ocr_uncertain == true`
/// - OR quality check says uncertain
/// - OR text is empty / missing
/// - OR text has bad characters
fn select_suspicious_nodes(page: &Page) -> Vec<(NodeId, &koharu_core::Transform, &TextData)> {
    page.nodes
        .iter()
        .filter_map(|(id, node)| match &node.kind {
            NodeKind::Text(td) => Some((*id, &node.transform, td)),
            _ => None,
        })
        .filter(|(_id, _tf, td)| is_suspicious(td, _tf))
        .collect()
}

fn is_suspicious(td: &TextData, tf: &koharu_core::Transform) -> bool {
    // Already flagged by previous quality check
    if td.ocr_uncertain {
        return true;
    }

    let text = td.text.as_deref().unwrap_or("");

    // Empty text
    if text.trim().is_empty() {
        return true;
    }

    // Bad characters
    if text.contains('□') || text.contains('�') {
        return true;
    }

    // Re-run quality check
    let report = assess_ocr_quality(OcrQualityInput {
        text: td.text.as_deref(),
        detector_confidence: td.confidence,
        ocr_confidence: td.ocr_confidence,
        bbox_width: tf.width,
        bbox_height: tf.height,
        is_vertical: matches!(td.source_direction, Some(TextDirection::Vertical)),
    });

    report.uncertain
}

/// Build ops that mark nodes uncertain without changing their text.
fn mark_uncertain_ops(
    page: PageId,
    nodes: &[(NodeId, &koharu_core::Transform, &TextData)],
) -> Vec<Op> {
    nodes
        .iter()
        .map(|(node_id, _, _)| Op::UpdateNode {
            page,
            id: *node_id,
            patch: NodePatch {
                data: Some(NodeDataPatch::Text(TextDataPatch {
                    ocr_uncertain: Some(true),
                    ..Default::default()
                })),
                transform: None,
                visible: None,
            },
            prev: NodePatch::default(),
        })
        .collect()
}

fn load_source_image(scene: &Scene, page: PageId, blobs: &BlobStore) -> Result<DynamicImage> {
    crate::pipeline::engines::support::load_source_image(scene, page, blobs)
        .context("failed to load source image")
}

fn encode_png(image: &DynamicImage) -> String {
    let mut buf = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG encoding should not fail in memory");
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}

/// Map context fields from an OCR response item into a `TextDataPatch` field.
/// Returns `Some(Some(...))` if at least one context field is present,
/// `None` if all are empty (skip the patch).
fn map_translation_context(
    item: &crate::pipeline::unlimited_ocr_client::UnlimitedOcrItem,
) -> Option<Option<TextTranslationContext>> {
    if item.role.is_none()
        && item.speaker_hint.is_none()
        && item.emotion_hint.is_none()
        && item.visual_hint.is_none()
        && item.translation_note.is_none()
        && !item.uncertain
    {
        return None;
    }
    Some(Some(TextTranslationContext {
        role: item.role.clone(),
        speaker_hint: item.speaker_hint.clone(),
        emotion_hint: item.emotion_hint.clone(),
        visual_hint: item.visual_hint.clone(),
        translation_note: item.translation_note.clone(),
        context_uncertain: item.uncertain,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use koharu_core::{Node, Transform};
    use uuid::Uuid;

    fn node_id(value: u128) -> NodeId {
        NodeId(Uuid::from_u128(value))
    }

    fn text_node(id: NodeId, text: Option<&str>, uncertain: bool) -> Node {
        Node {
            id,
            transform: Transform::default(),
            visible: true,
            kind: NodeKind::Text(TextData {
                text: text.map(str::to_string),
                ocr_uncertain: uncertain,
                ..Default::default()
            }),
        }
    }

    fn page_with(nodes: Vec<Node>) -> Page {
        let mut page = Page::new("test", 100, 100);
        page.nodes = nodes.into_iter().map(|n| (n.id, n)).collect();
        page
    }

    #[test]
    fn good_ocr_not_selected() {
        let page = page_with(vec![text_node(node_id(1), Some("こんにちは"), false)]);
        let selected = select_suspicious_nodes(&page);
        assert!(selected.is_empty());
    }

    #[test]
    fn empty_text_selected() {
        let page = page_with(vec![text_node(node_id(1), Some(""), false)]);
        let selected = select_suspicious_nodes(&page);
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn already_uncertain_selected() {
        let page = page_with(vec![text_node(node_id(1), Some("test"), true)]);
        let selected = select_suspicious_nodes(&page);
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn none_text_selected() {
        let page = page_with(vec![text_node(node_id(1), None, false)]);
        let selected = select_suspicious_nodes(&page);
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn bad_chars_selected() {
        let page = page_with(vec![text_node(node_id(1), Some("□abc"), false)]);
        let selected = select_suspicious_nodes(&page);
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn mixed_page_selects_only_bad() {
        let page = page_with(vec![
            text_node(node_id(1), Some("こんにちは"), false),
            text_node(node_id(2), Some(""), false),
            text_node(node_id(3), Some("ああああ"), false), // weird repetition
        ]);
        let selected = select_suspicious_nodes(&page);
        assert_eq!(selected.len(), 2);
        let ids: Vec<NodeId> = selected.iter().map(|(id, _, _)| *id).collect();
        assert!(ids.contains(&node_id(2)));
        assert!(ids.contains(&node_id(3)));
    }
}
