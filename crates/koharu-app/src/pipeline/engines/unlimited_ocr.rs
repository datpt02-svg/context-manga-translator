//! Unlimited-OCR engine. Crops each text box, sends the batch to the
//! Python Unlimited-OCR service via HTTP, and writes results back.

use anyhow::{Context, Result};
use async_trait::async_trait;
use image::DynamicImage;
use koharu_core::{NodeDataPatch, NodePatch, Op, TextDataPatch, TextTranslationContext};
use koharu_ml::comic_text_detector::crop_text_block_bbox;

use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo};
use crate::pipeline::engines::support::{load_source_image, text_node_to_region, text_nodes};
use base64::Engine as _;

use crate::pipeline::unlimited_ocr_client::{
    UnlimitedCropImage, UnlimitedCropRequest, UnlimitedOcrClient,
};

pub struct Model {
    client: UnlimitedOcrClient,
}

impl Model {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: UnlimitedOcrClient::new(base_url),
        }
    }
}

#[async_trait]
impl Engine for Model {
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>> {
        let texts = text_nodes(ctx.scene, ctx.page);
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let image = load_source_image(ctx.scene, ctx.page, ctx.blobs)
            .context("failed to load source image for Unlimited-OCR")?;

        // Build crop images
        let mut crop_images: Vec<UnlimitedCropImage> = Vec::with_capacity(texts.len());
        let mut nodes: Vec<(koharu_core::NodeId, &koharu_core::Transform, &koharu_core::TextData)> =
            Vec::with_capacity(texts.len());

        for (node_id, tf, td) in &texts {
            let region = text_node_to_region(tf, td);
            let crop = crop_text_block_bbox(&image, &region);
            let base64 = encode_png(&crop);

            crop_images.push(UnlimitedCropImage {
                id: node_id.to_string(),
                image_base64: base64,
            });
            nodes.push((*node_id, *tf, *td));
        }

        // Health check first
        self.client
            .health()
            .await
            .context("Unlimited-OCR service not available — is the Python service running?")?;

        // Send batch request
        let request = UnlimitedCropRequest {
            images: crop_images,
            language_hint: ctx.options.target_language.clone(),
            return_context: true,
        };

        let response = self
            .client
            .ocr_crops(request)
            .await
            .context("Unlimited-OCR batch request failed")?;

        // Build id → item map
        let mut item_map: std::collections::HashMap<String, &crate::pipeline::unlimited_ocr_client::UnlimitedOcrItem> =
            std::collections::HashMap::new();
        for item in &response.items {
            item_map.insert(item.id.clone(), item);
        }

        // Build ops
        let mut ops = Vec::with_capacity(nodes.len());
        for (node_id, _tf, _td) in nodes {
            let id_str = node_id.to_string();
            let item = match item_map.get(&id_str) {
                Some(item) => item,
                None => {
                    // Missing item in response — mark uncertain
                    ops.push(Op::UpdateNode {
                        page: ctx.page,
                        id: node_id,
                        patch: NodePatch {
                            data: Some(NodeDataPatch::Text(TextDataPatch {
                                ocr_engine: Some(Some("unlimited-ocr".to_string())),
                                ocr_uncertain: Some(true),
                                ..Default::default()
                            })),
                            transform: None,
                            visible: None,
                        },
                        prev: NodePatch::default(),
                    });
                    continue;
                }
            };

            let ctx_patch = map_translation_context(item);
            ops.push(Op::UpdateNode {
                page: ctx.page,
                id: node_id,
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(TextDataPatch {
                        text: Some(Some(item.text.clone())),
                        ocr_engine: Some(Some("unlimited-ocr".to_string())),
                        ocr_confidence: Some(item.confidence),
                        ocr_uncertain: Some(item.uncertain),
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
}

/// Encode a `DynamicImage` as a base64 PNG string.
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
