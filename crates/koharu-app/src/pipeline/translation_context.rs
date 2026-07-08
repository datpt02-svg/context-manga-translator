//! Translation context builder — enriches OCR text with layout metadata
//! so the LLM can produce higher-quality manga translations.

use koharu_core::{NodeId, NodeKind, PageId, ReadingOrder, Scene, TextData, TextDirection};
use serde::Serialize;

/// A single text block with layout/context metadata for LLM translation.
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

/// Build translation blocks from a page's text nodes.
///
/// Skips empty OCR text, respects `allowed_ids` filtering, computes
/// position from bbox center relative to page size, and fills
/// previous/next text based on reading order (or insertion order if
/// none specified).
pub fn build_translation_blocks(
    scene: &Scene,
    page: PageId,
    allowed_ids: Option<&[NodeId]>,
    reading_order: Option<ReadingOrder>,
) -> Vec<(NodeId, TranslationBlock)> {
    let Some(page_ref) = scene.page(page) else {
        return Vec::new();
    };

    // Collect: (id, bbox, text_data)
    let mut items: Vec<(NodeId, [f32; 4], &koharu_core::TextData)> = Vec::new();
    for (id, node) in &page_ref.nodes {
        let NodeKind::Text(t) = &node.kind else { continue };
        if t.text.as_deref().is_none_or(|s| s.trim().is_empty()) {
            continue;
        }
        if let Some(ids) = allowed_ids {
            if !ids.contains(id) {
                continue;
            }
        }
        let bbox = [
            node.transform.x,
            node.transform.y,
            node.transform.x + node.transform.width,
            node.transform.y + node.transform.height,
        ];
        items.push((*id, bbox, t));
    }

    if items.is_empty() {
        return Vec::new();
    }

    // Sort by reading order if specified
    if let Some(order) = reading_order {
        let mut sortable: Vec<([f32; 4], (NodeId, &TextData))> = items
            .iter()
            .map(|(id, bbox, td)| (*bbox, (*id, *td)))
            .collect();
        crate::pipeline::support::sort_manga_reading_order(&mut sortable, order);
        items = sortable
            .into_iter()
            .map(|(bbox, (id, td))| (id, bbox, td))
            .collect();
    }

    let page_width = page_ref.width as f32;
    let page_height = page_ref.height as f32;

    let mut blocks = Vec::with_capacity(items.len());
    for (i, (id, bbox, td)) in items.iter().enumerate() {
        let cx = (bbox[0] + bbox[2]) / 2.0;
        let cy = (bbox[1] + bbox[3]) / 2.0;
        let third_x = page_width / 3.0;
        let third_y = page_height / 3.0;

        let h_pos = if cx < third_x {
            "left"
        } else if cx < 2.0 * third_x {
            "center"
        } else {
            "right"
        };
        let v_pos = if cy < third_y {
            "top"
        } else if cy < 2.0 * third_y {
            "middle"
        } else {
            "bottom"
        };
        let position = format!("{}-{}", v_pos, h_pos);

        let source_direction = td.source_direction.map(|d| match d {
            TextDirection::Horizontal => "horizontal",
            TextDirection::Vertical => "vertical",
        }.to_string());

        let previous_text = if i > 0 {
            items[i - 1].2.text.clone()
        } else {
            None
        };
        let next_text = items.get(i + 1).and_then(|(_, _, t)| t.text.clone());

        blocks.push((
            *id,
            TranslationBlock {
                id: id.to_string(),
                text: td.text.clone().unwrap_or_default(),
                order: i,
                bbox: *bbox,
                position,
                source_direction,
                detector_confidence: td.confidence,
                detector: td.detector.clone(),
                ocr_engine: td.ocr_engine.clone(),
                ocr_confidence: td.ocr_confidence,
                ocr_uncertain: td.ocr_uncertain,
                role: td.translation_context.as_ref().and_then(|c| c.role.clone()),
                speaker_hint: td.translation_context.as_ref().and_then(|c| c.speaker_hint.clone()),
                emotion_hint: td.translation_context.as_ref().and_then(|c| c.emotion_hint.clone()),
                visual_hint: td.translation_context.as_ref().and_then(|c| c.visual_hint.clone()),
                translation_note: td.translation_context.as_ref().and_then(|c| c.translation_note.clone()),
                previous_text,
                next_text,
            },
        ));
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use koharu_core::{Node, Page, Transform};
    use uuid::Uuid;

    fn node_id(value: u128) -> NodeId {
        NodeId(Uuid::from_u128(value))
    }

    fn page_id() -> PageId {
        PageId(Uuid::from_u128(1))
    }

    fn text_node(
        id: NodeId,
        text: Option<&str>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> Node {
        Node {
            id,
            transform: Transform { x, y, width: w, height: h, rotation_deg: 0.0 },
            visible: true,
            kind: NodeKind::Text(TextData {
                text: text.map(str::to_string),
                confidence: 0.95,
                ..Default::default()
            }),
        }
    }

    fn scene_with(nodes: Vec<Node>, width: u32, height: u32) -> Scene {
        let pid = page_id();
        let mut page = Page::new("test", width, height);
        page.id = pid;
        page.nodes = nodes.into_iter().map(|n| (n.id, n)).collect();
        let mut scene = Scene::default();
        scene.pages.insert(pid, page);
        scene
    }

    #[test]
    fn skip_blank_ocr_text() {
        let scene = scene_with(
            vec![
                text_node(node_id(1), Some(" "), 0.0, 0.0, 10.0, 10.0),
                text_node(node_id(2), Some("valid"), 0.0, 0.0, 10.0, 10.0),
            ],
            100,
            100,
        );
        let blocks = build_translation_blocks(&scene, page_id(), None, None);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].1.text, "valid");
    }

    #[test]
    fn include_bbox() {
        let scene = scene_with(
            vec![text_node(node_id(1), Some("hello"), 10.0, 20.0, 100.0, 30.0)],
            500,
            500,
        );
        let blocks = build_translation_blocks(&scene, page_id(), None, None);
        assert_eq!(blocks[0].1.bbox, [10.0, 20.0, 110.0, 50.0]);
    }

    #[test]
    fn compute_position_correctly() {
        // bbox centered at (55, 25) on page 500×500
        let scene = scene_with(
            vec![text_node(node_id(1), Some("left-top"), 0.0, 0.0, 50.0, 50.0)],
            500,
            500,
        );
        let blocks = build_translation_blocks(&scene, page_id(), None, None);
        assert_eq!(blocks[0].1.position, "top-left");

        // bbox centered at (350, 350) → bottom-right
        let scene2 = scene_with(
            vec![text_node(node_id(2), Some("br"), 300.0, 300.0, 100.0, 100.0)],
            500,
            500,
        );
        let blocks2 = build_translation_blocks(&scene2, page_id(), None, None);
        assert_eq!(blocks2[0].1.position, "bottom-right");
    }

    #[test]
    fn include_previous_next_text() {
        let scene = scene_with(
            vec![
                text_node(node_id(1), Some("first"), 0.0, 0.0, 10.0, 10.0),
                text_node(node_id(2), Some("second"), 50.0, 0.0, 10.0, 10.0),
            ],
            100,
            100,
        );
        let blocks = build_translation_blocks(&scene, page_id(), None, None);
        assert_eq!(blocks.len(), 2);
        // First block has no previous, next = "second"
        assert!(blocks[0].1.previous_text.is_none());
        assert_eq!(blocks[0].1.next_text.as_deref(), Some("second"));
        // Second block has previous = "first", no next
        assert_eq!(blocks[1].1.previous_text.as_deref(), Some("first"));
        assert!(blocks[1].1.next_text.is_none());
    }

    #[test]
    fn respect_allowed_text_node_ids() {
        let allowed = vec![node_id(2)];
        let scene = scene_with(
            vec![
                text_node(node_id(1), Some("skip"), 0.0, 0.0, 10.0, 10.0),
                text_node(node_id(2), Some("keep"), 0.0, 0.0, 10.0, 10.0),
                text_node(node_id(3), Some("skip"), 0.0, 0.0, 10.0, 10.0),
            ],
            100,
            100,
        );
        let blocks =
            build_translation_blocks(&scene, page_id(), Some(&allowed), None);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].1.text, "keep");
    }
}
