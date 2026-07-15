//! vLLM OCR engine. Crops each text box and sends it through a vLLM vision
//! model via the OpenAI-compatible `/v1/chat/completions` endpoint.
//!
//! ## Configuration priority
//!
//! 1. Saved pipeline config (settings → pipeline → vLLM OCR target).
//! 2. Environment variables (`VLLM_OCR_*`) — CLI/back-compat fallback.
//!
//! Sends one request per crop (simplest mapping), parses
//! `choices[0].message.content`.

use anyhow::{Context, Result};
use tracing;
use async_trait::async_trait;
use base64::Engine as _;
use futures::StreamExt;
use image::DynamicImage;
use koharu_core::{NodeDataPatch, NodeId, NodePatch, Op, TextDataPatch, TextDirection};
use koharu_ml::comic_text_detector::crop_text_block_bbox;

use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo, PipelineRunOptions};
use crate::pipeline::engines::support::{load_source_image, text_node_to_region, text_nodes};
use crate::pipeline::ocr_quality::{OcrQualityInput, assess_ocr_quality};

const DEFAULT_MAX_TOKENS: u32 = 20000;

/// Resolved connection parameters for a vLLM OCR run.
struct VllmOcrSettings {
    model: String,
    base_url: String,
    api_key: Option<String>,
    max_tokens: u32,
    temperature: f64,
    system_prompt: String,
}

impl VllmOcrSettings {
    fn resolve(opts: &PipelineRunOptions) -> Result<Self> {
        let model = opts
            .vllm_ocr_model
            .clone()
            .or_else(|| std::env::var("VLLM_OCR_MODEL").ok().filter(|v| !v.trim().is_empty()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "vLLM OCR not configured: set a model in Settings → Engines → OCR, \
                     or VLLM_OCR_MODEL env var"
                )
            })?;

        let base_url = opts
            .vllm_ocr_base_url
            .clone()
            .or_else(|| std::env::var("VLLM_OCR_BASE_URL").ok())
            .unwrap_or_else(|| "http://127.0.0.1:8000/v1".to_string());
        let base_url = base_url.trim().trim_end_matches('/').to_string();

        let api_key = opts
            .vllm_ocr_api_key
            .clone()
            .or_else(|| std::env::var("VLLM_OCR_API_KEY").ok())
            .filter(|v| !v.trim().is_empty());

        let max_tokens = opts
            .vllm_ocr_max_tokens
            .or_else(|| {
                std::env::var("VLLM_OCR_MAX_TOKENS")
                    .ok()
                    .and_then(|v| v.trim().parse().ok())
            })
            .unwrap_or(DEFAULT_MAX_TOKENS);

        let temperature = opts
            .vllm_ocr_temperature
            .unwrap_or(0.0);

        let target_lang = opts
            .vllm_ocr_target_language
            .as_deref()
            .or_else(|| opts.target_language.as_deref())
            .unwrap_or("the target language");
        let system_prompt = opts
            .vllm_ocr_system_prompt
            .clone()
            .filter(|s| !s.trim().is_empty())
            .map(|p| p.replace("{{ target_language }}", target_lang))
            .unwrap_or_else(|| format!("You are a professional manga translator. Read the Japanese text in this image and translate it into {target_lang}. Return a JSON object with two fields: \"ocr\" (the original Japanese text) and \"translation\" (the {target_lang} translation). Example: {{\"ocr\":\"元気ですか？\",\"translation\":\"Khỏe không?\"}}"));

        eprintln!(
            "[vllm_ocr] resolved: model={model}, base_url={base_url}, target_lang={target_lang}, prompt={}",
            system_prompt,
        );

        Ok(Self { model, base_url, api_key, max_tokens, temperature, system_prompt })
    }
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

pub struct Model {
    client: reqwest::Client,
}

impl Model {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Engine for Model {
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>> {
        let texts = text_nodes(ctx.scene, ctx.page);
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let settings = VllmOcrSettings::resolve(ctx.options)?;
        let endpoint = format!("{}/chat/completions", settings.base_url);
        let image = load_source_image(ctx.scene, ctx.page, ctx.blobs)?;

        // Phase 1 — synchronous prep: crop + base64 encode every text box.
        struct CropJob {
            node_id: NodeId,
            b64: String,
            confidence: f32,
            is_vertical: bool,
            bbox_width: f32,
            bbox_height: f32,
        }
        let jobs: Vec<CropJob> = texts
            .iter()
            .map(|(node_id, tf, td)| {
                let region = text_node_to_region(tf, td);
                let crop = crop_text_block_bbox(&image, &region);
                let b64 = encode_png(&crop);
                CropJob {
                    node_id: *node_id,
                    b64,
                    confidence: td.confidence,
                    is_vertical: matches!(
                        td.source_direction,
                        Some(TextDirection::Vertical)
                    ),
                    bbox_width: tf.width,
                    bbox_height: tf.height,
                }
            })
            .collect::<Vec<_>>();

        // Phase 2 — concurrent OCR requests (vLLM batches internally).
        let concurrency = 2usize;
        let results: Vec<(usize, NodeId, f32, bool, f32, f32, Result<String>)> =
            futures::stream::iter(jobs.into_iter().enumerate().map(
                |(i, job)| {
                    let endpoint = endpoint.clone();
                    let model = settings.model.clone();
                    let api_key = settings.api_key.clone();
                    let system_prompt = settings.system_prompt.clone();
                    async move {
                        let text = ocr_one_crop(
                            &self.client,
                            &endpoint,
                            &model,
                            api_key.as_deref(),
                            settings.max_tokens,
                            settings.temperature,
                            &system_prompt,
                            &job.b64,
                        )
                        .await;
                        (
                            i,
                            job.node_id,
                            job.confidence,
                            job.is_vertical,
                            job.bbox_width,
                            job.bbox_height,
                            text,
                        )
                    }
                },
            ))
            .buffer_unordered(concurrency)
            .collect()
            .await;

        // Phase 3 — build ops (results arrive in arbitrary order, each
        // Op targets a specific node_id so ordering is irrelevant).
        let mut ops = Vec::with_capacity(texts.len());
        for (_i, node_id, td_confidence, is_vertical, bbox_width, bbox_height, result) in results {
            let recognized = match result {
                Ok(text) => text,
                Err(e) => {
                    ops.push(Op::UpdateNode {
                        page: ctx.page,
                        id: node_id,
                        patch: NodePatch {
                            data: Some(NodeDataPatch::Text(TextDataPatch {
                                ocr_engine: Some(Some("vllm-ocr".to_string())),
                                ocr_uncertain: Some(true),
                                ..Default::default()
                            })),
                            transform: None,
                            visible: None,
                        },
                        prev: NodePatch::default(),
                    });
                    tracing::warn!("vLLM OCR crop {node_id} failed: {e:#}");
                    continue;
                }
            };

            if recognized.trim().is_empty() {
                ops.push(Op::UpdateNode {
                    page: ctx.page,
                    id: node_id,
                    patch: NodePatch {
                        data: Some(NodeDataPatch::Text(TextDataPatch {
                            ocr_engine: Some(Some("vllm-ocr".to_string())),
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

            let (ocr_text, translation) = match serde_json::from_str::<serde_json::Value>(&recognized) {
                Ok(v) => (
                    v["ocr"].as_str().unwrap_or(&recognized).to_string(),
                    v["translation"].as_str().unwrap_or(&recognized).to_string(),
                ),
                _ => (recognized.clone(), recognized.clone()),
            };

            let report = assess_ocr_quality(OcrQualityInput {
                text: Some(&ocr_text),
                detector_confidence: td_confidence,
                ocr_confidence: None,
                bbox_width,
                bbox_height,
                is_vertical,
            });

            ops.push(Op::UpdateNode {
                page: ctx.page,
                id: node_id,
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(TextDataPatch {
                        text: Some(Some(ocr_text)),
                        translation: Some(Some(translation)),
                        ocr_engine: Some(Some("vllm-ocr".to_string())),
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

// ---------------------------------------------------------------------------
// OpenAI-compatible multimodal request
// ---------------------------------------------------------------------------

/// Send one image crop through vLLM's multimodal chat endpoint.
async fn ocr_one_crop(
    client: &reqwest::Client,
    endpoint: &str,
    model: &str,
    api_key: Option<&str>,
    max_tokens: u32,
    temperature: f64,
    system_prompt: &str,
    image_b64: &str,
) -> Result<String> {
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": system_prompt },
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:image/png;base64,{image_b64}")
                        }
                    }
                ]
            }
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
    });

    let mut req = client.post(endpoint).header("content-type", "application/json");
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }

    let resp: serde_json::Value = req
        .body(serde_json::to_vec(&body)?)
        .send()
        .await
        .context("vLLM OCR HTTP request failed")?
        .error_for_status()
        .context("vLLM OCR HTTP error")?
        .json()
        .await?;

    let raw = &resp["choices"][0]["message"];
    let content = raw["content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| raw["reasoning_content"].as_str())
        .ok_or_else(|| anyhow::anyhow!("vLLM OCR response has no content or reasoning_content"))?
        .to_string();

    // Try to parse as JSON {"ocr": "...", "translation": "..."}. Fallback: use raw content as both.
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
        let ocr_text = parsed["ocr"].as_str().unwrap_or(&content).to_string();
        let translation = parsed["translation"].as_str().unwrap_or(&ocr_text).to_string();
        return Ok(serde_json::json!({"ocr": ocr_text, "translation": translation}).to_string());
    }
    Ok(content)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Encode a `DynamicImage` as a base64 PNG string.
fn encode_png(image: &DynamicImage) -> String {
    let mut buf = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG encoding should not fail in memory");
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

inventory::submit! {
    EngineInfo {
        id: "vllm-ocr",
        name: "vLLM OCR",
        needs: &[Artifact::TextBoxes],
        produces: &[Artifact::OcrText, Artifact::Translations],
        load: |_runtime, _cpu| Box::pin(async move {
            Ok(Box::new(Model::new()) as Box<dyn Engine>)
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    #[test]
    fn png_base64_roundtrip() {
        let img = DynamicImage::new_rgba8(4, 4);
        let b64 = encode_png(&img);
        assert!(!b64.is_empty());
        // decode back
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .expect("base64 decode");
        let decoded = image::load_from_memory(&bytes).expect("PNG decode");
        assert_eq!(decoded.dimensions(), (4, 4));
    }

    #[test]
    fn parse_valid_response() {
        let json = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "これはテストです"
                }
            }]
        });
        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap();
        assert_eq!(content, "これはテストです");
    }

    #[test]
    fn parse_empty_response_field() {
        let json = serde_json::json!({
            "choices": [{
                "message": {
                    "content": null
                }
            }]
        });
        assert!(json["choices"][0]["message"]["content"].as_str().is_none());
    }
}
