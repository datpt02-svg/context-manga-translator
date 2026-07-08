//! Manga OCR. Each text node's bbox is cropped and sent through a small
//! CRNN; the recognised text is written back via `UpdateNode`.

use anyhow::Result;
use async_trait::async_trait;
use image::DynamicImage;
use koharu_core::{NodeDataPatch, NodePatch, Op, TextDataPatch, TextDirection};
use koharu_ml::comic_text_detector::crop_text_block_bbox;
use koharu_ml::manga_ocr::MangaOcr;

use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo};
use crate::pipeline::engines::support::{load_source_image, text_node_to_region, text_nodes};
use crate::pipeline::ocr_quality::{OcrQualityInput, assess_ocr_quality};

pub struct Model(MangaOcr);

#[async_trait]
impl Engine for Model {
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>> {
        let texts = text_nodes(ctx.scene, ctx.page);
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let image = load_source_image(ctx.scene, ctx.page, ctx.blobs)?;
        let crops: Vec<DynamicImage> = texts
            .iter()
            .map(|(_, transform, text)| {
                let region = text_node_to_region(transform, text);
                crop_text_block_bbox(&image, &region)
            })
            .collect();
        let recognised = self.0.inference(&crops)?;

        let mut ops = Vec::with_capacity(texts.len());
        for ((node_id, tf, td), text) in texts.iter().zip(recognised) {
            let report = assess_ocr_quality(OcrQualityInput {
                text: Some(&text),
                detector_confidence: td.confidence,
                ocr_confidence: None,
                bbox_width: tf.width,
                bbox_height: tf.height,
                is_vertical: matches!(td.source_direction, Some(TextDirection::Vertical)),
            });
            ops.push(Op::UpdateNode {
                page: ctx.page,
                id: *node_id,
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(TextDataPatch {
                        text: Some(Some(text)),
                        ocr_engine: Some(Some("manga-ocr".to_string())),
                        ocr_confidence: Some(None),
                        ocr_uncertain: Some(report.uncertain),
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

inventory::submit! {
    EngineInfo {
        id: "manga-ocr",
        name: "Manga OCR",
        needs: &[Artifact::TextBoxes],
        produces: &[Artifact::OcrText],
        load: |runtime, cpu| Box::pin(async move {
            let m = MangaOcr::load(runtime, cpu).await?;
            Ok(Box::new(Model(m)) as Box<dyn Engine>)
        }),
    }
}
