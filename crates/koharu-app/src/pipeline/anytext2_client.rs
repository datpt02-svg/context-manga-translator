//! HTTP client for the AnyText2 Python render service.
//!
//! Sends per-block crops + translations and receives rendered sprites
//! via POST /render.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontHint {
    pub serif: bool,
    pub language: Option<String>,
    pub family: Option<String>,
    pub font_size_px: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBlock {
    pub id: String,
    pub translation: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub source_crop_base64: String,
    pub inpainted_crop_base64: String,
    pub text_color: Vec<u8>,
    pub font_hint: Option<FontHint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderRequest {
    pub image_width: u32,
    pub image_height: u32,
    pub source_image_base64: String,
    pub inpainted_image_base64: String,
    pub blocks: Vec<TextBlock>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedBlock {
    pub id: String,
    pub rendered_crop_base64: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderResponse {
    pub blocks: Vec<RenderedBlock>,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

static RENDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
static HEALTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct AnyText2Client {
    client: reqwest::Client,
    base_url: String,
}

impl AnyText2Client {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }

    /// Health check — returns Ok if the service is reachable and model loaded.
    pub async fn health(&self) -> Result<()> {
        let url = format!("{}/health", self.base_url);
        self.client
            .get(&url)
            .timeout(HEALTH_TIMEOUT)
            .send()
            .await
            .with_context(|| {
                format!(
                    "AnyText2 health check failed (service at {})",
                    self.base_url
                )
            })?;
        Ok(())
    }

    /// Send a batch of blocks for text rendering.
    pub async fn render(&self, req: RenderRequest) -> Result<RenderResponse> {
        let url = format!("{}/render", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&req)
            .timeout(RENDER_TIMEOUT)
            .send()
            .await
            .with_context(|| {
                format!(
                    "AnyText2 request failed (service at {}, {} blocks)",
                    self.base_url,
                    req.blocks.len(),
                )
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "AnyText2 service returned {status} (service at {}): {body}",
                self.base_url,
            );
        }

        resp.json::<RenderResponse>()
            .await
            .context("failed to parse AnyText2 response")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_trim_trailing_slash() {
        let client = AnyText2Client::new("http://example.com/");
        assert_eq!(client.base_url, "http://example.com");
    }

    #[test]
    fn request_json_shape() {
        let req = RenderRequest {
            image_width: 1920,
            image_height: 2560,
            source_image_base64: "fake".to_string(),
            inpainted_image_base64: "fake".to_string(),
            blocks: vec![TextBlock {
                id: "n1".to_string(),
                translation: "Hello".to_string(),
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0,
                source_crop_base64: "crop".to_string(),
                inpainted_crop_base64: "crop2".to_string(),
                text_color: vec![0, 0, 0, 255],
                font_hint: Some(FontHint {
                    serif: false,
                    language: Some("ja".to_string()),
                    family: None,
                    font_size_px: Some(24.0),
                }),
            }],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["blocks"][0]["id"], "n1");
        assert_eq!(json["blocks"][0]["translation"], "Hello");
        assert_eq!(json["blocks"][0]["fontHint"]["language"], "ja");
    }

    #[test]
    fn response_deserialize() {
        let raw = r#"{
            "blocks": [
                {
                    "id": "n1",
                    "renderedCropBase64": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                }
            ],
            "warnings": []
        }"#;
        let resp: RenderResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.blocks.len(), 1);
        assert_eq!(resp.blocks[0].id, "n1");
        assert!(!resp.blocks[0].rendered_crop_base64.is_empty());
    }
}
