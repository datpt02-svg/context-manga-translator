//! LLM-driven translation. Two paths:
//!
//! 1. **Rich context** — builds `TranslationBlock` with position, bbox,
//!    reading order, previous/next text, and source direction, then calls
//!    the LLM with a structured JSON prompt.
//! 2. **Plain fallback** — sends raw `[N]tagged` strings (the old path).
//!
//! The rich path is attempted first. If it fails at any point (JSON parse
//! error, LLM error, unexpected output shape) the engine falls back to the
//! old plain-text path.

use anyhow::{Context, Result};
use async_trait::async_trait;
use koharu_core::{NodeDataPatch, NodeId, NodePatch, Op, PageId, Scene, TextData, TextDataPatch};
use serde_json::Value;

use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo};
use crate::pipeline::engines::support::text_nodes;
use crate::pipeline::translation_context::{TranslationBlock, build_translation_blocks};

pub struct Model;

#[async_trait]
impl Engine for Model {
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>> {
        // === Rich context path (attempted first) ===
        let blocks = build_translation_blocks(
            ctx.scene,
            ctx.page,
            ctx.options.text_node_ids.as_deref(),
            ctx.options.reading_order,
        );

        if blocks.is_empty() {
            return Ok(Vec::new());
        }

        // Extract just the serializable blocks (without NodeIds).
        let block_list: Vec<&TranslationBlock> = blocks.iter().map(|(_, b)| b).collect();

        match translate_with_rich_context(
            ctx.llm,
            &block_list,
            ctx.options.target_language.as_deref(),
            ctx.options.system_prompt.as_deref(),
        )
        .await
        {
            Ok(translations) => {
                // Map translations by id
                return Ok(build_ops_from_id_map(ctx.page, &blocks, &translations));
            }
            Err(e) => {
                tracing::warn!(
                    "rich context translation failed, falling back to plain text: {e:#}"
                );
                // Fall through to plain text path.
            }
        }

        // === Plain text fallback path ===
        let targets = collect_translation_targets(&ctx);
        if targets.is_empty() {
            return Ok(Vec::new());
        }

        let sources: Vec<String> = targets.iter().map(|(_, s)| s.clone()).collect();
        let translations = ctx
            .llm
            .translate_texts(
                &sources,
                ctx.options.target_language.as_deref(),
                ctx.options.system_prompt.as_deref(),
            )
            .await?;

        let mut ops = Vec::with_capacity(targets.len());
        for ((node_id, _), translation) in targets.into_iter().zip(translations) {
            ops.push(Op::UpdateNode {
                page: ctx.page,
                id: node_id,
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(TextDataPatch {
                        translation: Some(Some(translation)),
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

/// Attempt rich-context translation.
///
/// 1. Serialize blocks to a JSON payload.
/// 2. Call LLM with a JSON-output system prompt.
/// 3. Parse `[{"id":"...","translation":"..."}]` response.
/// 4. Map back by id.
async fn translate_with_rich_context(
    llm: &crate::llm::Model,
    blocks: &[&TranslationBlock],
    target_language: Option<&str>,
    system_prompt: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let json_blocks = serde_json::to_value(blocks)?;

    let payload = serde_json::json!({
        "targetLanguage": target_language.unwrap_or("Vietnamese"),
        "blocks": json_blocks,
    });

    let prompt = serde_json::to_string_pretty(&payload)
        .context("failed to serialize translation payload")?;

    let rich_system = system_prompt.map(|s| s.to_string()).unwrap_or_else(|| {
        "You are a professional Japanese-to-Vietnamese manga translator.\n\
             Translate each Japanese manga text box naturally into Vietnamese.\n\
             Use reading order, previous/next text, position, and source direction as context.\n\
             If available, consider role (dialogue/narration/SFX), speaker hints, emotion hints,\n\
             and visual hints as soft context. If hints conflict with the Japanese text,\n\
             prioritize the Japanese text.\n\
             Preserve tone, brevity, and manga style.\n\
             Return strict JSON only:\n\
             [\n  {\"id\":\"...\", \"translation\":\"...\"}\n]"
            .to_string()
    });

    let response = llm
        .translate_raw(&prompt, Some(&rich_system), target_language)
        .await
        .context("rich context LLM call failed")?;

    parse_translation_json(&response, blocks.len())
}

/// Parse `[{"id":"...","translation":"..."}]` from the LLM response.
///
/// Fallback rules:
/// - If JSON parse succeeds and all ids match, return by id.
/// - If output length equals input length, fallback to positional index.
/// - Otherwise fail.
fn parse_translation_json(response: &str, expected_count: usize) -> Result<Vec<(String, String)>> {
    // Try to find a JSON array in the response (LLMs sometimes wrap in markdown).
    let body = extract_json_array(response).unwrap_or(response.trim());

    let parsed: Vec<Value> =
        serde_json::from_str(body).context("failed to parse LLM JSON response")?;

    if parsed.is_empty() {
        anyhow::bail!("LLM returned empty translation array");
    }

    // Try id-based mapping first.
    let all_have_ids = parsed
        .iter()
        .all(|v| v.get("id").and_then(|i| i.as_str()).is_some());
    if all_have_ids {
        let result: Vec<(String, String)> = parsed
            .iter()
            .filter_map(|v| {
                let id = v["id"].as_str()?;
                let translation = v["translation"].as_str()?;
                Some((id.to_string(), translation.to_string()))
            })
            .collect();
        if !result.is_empty() {
            return Ok(result);
        }
    }

    // Positional fallback: if output count matches input count, use array index.
    if parsed.len() == expected_count {
        let result: Vec<(String, String)> = parsed
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let translation = v["translation"].as_str()?;
                Some((i.to_string(), translation.to_string()))
            })
            .collect();
        if result.len() == expected_count {
            return Ok(result);
        }
    }

    anyhow::bail!(
        "JSON parse mismatch: expected {} items, got {}",
        expected_count,
        parsed.len()
    );
}

/// Extract the first JSON array `[...]` from text that may contain markdown
/// fences around it.
fn extract_json_array(text: &str) -> Option<&str> {
    // Try extracting from ```json ... ``` blocks first.
    if let Some(start) = text.find("```json") {
        let after = &text[start + 7..];
        if let Some(end) = after.find("```") {
            let candidate = after[..end].trim();
            if candidate.starts_with('[') {
                return Some(candidate);
            }
        }
    }
    // Try plain code fences.
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        if let Some(end) = after.find("```") {
            let candidate = after[..end].trim();
            if candidate.starts_with('[') {
                return Some(candidate);
            }
        }
    }
    // Look for top-level `[` ... `]` in the whole text.
    if let Some(start) = text.find('[') {
        // Walk forward to find the matching `]`.
        let remainder = &text[start..];
        let mut depth = 0;
        for (i, ch) in remainder.char_indices() {
            match ch {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&remainder[..=i]);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Build update ops by mapping translation id back to NodeId.
/// If a block's id has no matching translation, its translation is left unchanged.
fn build_ops_from_id_map(
    page: PageId,
    blocks: &[(NodeId, TranslationBlock)],
    translations: &[(String, String)],
) -> Vec<Op> {
    let mut id_map = std::collections::HashMap::new();
    for (id_str, trans) in translations {
        id_map.insert(id_str.clone(), trans.clone());
    }

    blocks
        .iter()
        .filter_map(|(node_id, block)| {
            let translation = id_map.get(&block.id)?.clone();
            Some(Op::UpdateNode {
                page,
                id: *node_id,
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(TextDataPatch {
                        translation: Some(Some(translation)),
                        ..Default::default()
                    })),
                    transform: None,
                    visible: None,
                },
                prev: NodePatch::default(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Plain text helpers (kept for fallback)
// ---------------------------------------------------------------------------

fn collect_translation_targets(ctx: &EngineCtx<'_>) -> Vec<(NodeId, String)> {
    collect_translation_targets_from(ctx.scene, ctx.page, ctx.options.text_node_ids.as_deref())
}

fn collect_translation_targets_from(
    scene: &Scene,
    page: PageId,
    allowed_ids: Option<&[NodeId]>,
) -> Vec<(NodeId, String)> {
    text_nodes(scene, page)
        .into_iter()
        .filter(|(id, _, text_data)| should_translate(*id, text_data, allowed_ids))
        .filter_map(|(id, _, text_data)| text_data.text.as_ref().map(|source| (id, source.clone())))
        .collect()
}

fn should_translate(id: NodeId, text_data: &TextData, allowed_ids: Option<&[NodeId]>) -> bool {
    if let Some(ids) = allowed_ids
        && !ids.contains(&id)
    {
        return false;
    }
    text_data
        .text
        .as_ref()
        .is_some_and(|source| !source.trim().is_empty())
}

inventory::submit! {
    EngineInfo {
        id: "llm",
        name: "LLM",
        needs: &[Artifact::OcrText],
        produces: &[Artifact::Translations],
        load: |_runtime, _cpu| Box::pin(async move {
            Ok(Box::new(Model) as Box<dyn Engine>)
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use koharu_core::{Node, NodeKind, Page, Scene, TextData, Transform};
    use uuid::Uuid;

    fn node_id(value: u128) -> NodeId {
        NodeId(Uuid::from_u128(value))
    }

    fn page_id() -> PageId {
        PageId(Uuid::from_u128(1))
    }

    fn text_node(id: NodeId, text: Option<&str>, x: f32, y: f32, w: f32, h: f32) -> Node {
        Node {
            id,
            transform: Transform {
                x,
                y,
                width: w,
                height: h,
                rotation_deg: 0.0,
            },
            visible: true,
            kind: NodeKind::Text(TextData {
                text: text.map(str::to_string),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn parse_id_based_translations() {
        let response = r#"[
            {"id": "abc", "translation": "hello"},
            {"id": "def", "translation": "world"}
        ]"#;
        let result = parse_translation_json(response, 2).unwrap();
        assert_eq!(
            result,
            vec![
                ("abc".to_string(), "hello".to_string()),
                ("def".to_string(), "world".to_string()),
            ]
        );
    }

    #[test]
    fn parse_json_extracted_from_markdown_fence() {
        let response =
            "Here is the translation:\n```json\n[{\"id\":\"x\",\"translation\":\"test\"}]\n```\n";
        let result = parse_translation_json(response, 1).unwrap();
        assert_eq!(result, vec![("x".to_string(), "test".to_string())]);
    }

    #[test]
    fn positional_fallback_when_no_ids() {
        let response = r#"[
            {"translation": "first"},
            {"translation": "second"}
        ]"#;
        let result = parse_translation_json(response, 2).unwrap();
        assert_eq!(
            result,
            vec![
                ("0".to_string(), "first".to_string()),
                ("1".to_string(), "second".to_string()),
            ]
        );
    }

    #[test]
    fn empty_response_fails() {
        let result = parse_translation_json("[]", 1);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_json_fails() {
        let result = parse_translation_json("not json at all", 1);
        assert!(result.is_err());
    }

    #[test]
    fn extract_json_array_detects_brackets() {
        assert_eq!(extract_json_array("[1,2,3]"), Some("[1,2,3]"));
        assert_eq!(extract_json_array("text [1] more"), Some("[1]"));
        assert_eq!(
            extract_json_array("```json\n[{\"a\":1}]\n```"),
            Some("[{\"a\":1}]")
        );
    }

    #[test]
    fn should_translate_only_requested_nodes() {
        let first = node_id(11);
        let second = node_id(22);
        let scene = scene_with_texts(vec![
            text_node(first, Some("first"), 0.0, 0.0, 10.0, 10.0),
            text_node(second, Some("second"), 0.0, 0.0, 10.0, 10.0),
        ]);
        let options = crate::PipelineRunOptions {
            text_node_ids: Some(vec![second]),
            ..Default::default()
        };

        let targets =
            collect_translation_targets_from(&scene, page_id(), options.text_node_ids.as_deref());

        assert_eq!(targets, vec![(second, "second".to_string())]);
    }

    #[test]
    fn should_ignore_requested_nodes_without_ocr_text() {
        let blank = node_id(33);
        let scene = scene_with_texts(vec![
            text_node(blank, Some("   "), 0.0, 0.0, 10.0, 10.0),
            text_node(node_id(44), Some("translated"), 0.0, 0.0, 10.0, 10.0),
        ]);
        let options = crate::PipelineRunOptions {
            text_node_ids: Some(vec![blank]),
            ..Default::default()
        };

        let targets =
            collect_translation_targets_from(&scene, page_id(), options.text_node_ids.as_deref());

        assert!(targets.is_empty());
    }

    fn scene_with_texts(nodes: Vec<Node>) -> Scene {
        let pid = page_id();
        let mut page = Page::new("page", 100, 100);
        page.id = pid;
        page.nodes = nodes.into_iter().map(|node| (node.id, node)).collect();
        let mut scene = Scene::default();
        scene.pages.insert(pid, page);
        scene
    }
}
