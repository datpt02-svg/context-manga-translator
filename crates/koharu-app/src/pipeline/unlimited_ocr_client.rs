//! HTTP client for the Unlimited-OCR Python service.
//!
//! Sends cropped text-box images via POST /ocr/crops and receives
//! recognised text + metadata.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlimitedCropImage {
    pub id: String,
    pub image_base64: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlimitedCropRequest {
    pub images: Vec<UnlimitedCropImage>,
    pub language_hint: Option<String>,
    pub return_context: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlimitedOcrResponse {
    pub items: Vec<UnlimitedOcrItem>,
    pub page_context: Option<serde_json::Value>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlimitedOcrItem {
    pub id: String,
    pub text: String,
    pub confidence: Option<f32>,
    pub uncertain: bool,
    pub role: Option<String>,
    pub speaker_hint: Option<String>,
    pub emotion_hint: Option<String>,
    pub visual_hint: Option<String>,
    pub translation_note: Option<String>,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

static OCR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
static HEALTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct UnlimitedOcrClient {
    client: reqwest::Client,
    base_url: String,
}

impl UnlimitedOcrClient {
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
                    "Unlimited-OCR health check failed (service at {})",
                    self.base_url
                )
            })?;
        Ok(())
    }

    /// Send a batch of cropped images for OCR.
    pub async fn ocr_crops(&self, req: UnlimitedCropRequest) -> Result<UnlimitedOcrResponse> {
        let url = format!("{}/ocr/crops", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&req)
            .timeout(OCR_TIMEOUT)
            .send()
            .await
            .with_context(|| {
                format!(
                    "Unlimited-OCR request failed (service at {}, {} crops)",
                    self.base_url,
                    req.images.len(),
                )
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Unlimited-OCR service returned {status} (service at {}): {body}",
                self.base_url,
            );
        }

        resp.json::<UnlimitedOcrResponse>()
            .await
            .context("failed to parse Unlimited-OCR response")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_trim_trailing_slash() {
        let client = UnlimitedOcrClient::new("http://example.com/");
        assert_eq!(client.base_url, "http://example.com");
    }

    #[test]
    fn request_json_shape() {
        let req = UnlimitedCropRequest {
            images: vec![UnlimitedCropImage {
                id: "n1".to_string(),
                image_base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAA=".to_string(),
            }],
            language_hint: Some("ja".to_string()),
            return_context: true,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["images"][0]["id"], "n1");
        assert_eq!(json["languageHint"], "ja");
        assert!(json["returnContext"].as_bool().unwrap());
    }

    #[test]
    fn response_deserialize() {
        let raw = r#"{
            "items": [
                {
                    "id": "n1",
                    "text": "こんにちは",
                    "confidence": null,
                    "uncertain": false,
                    "role": null,
                    "speakerHint": null,
                    "emotionHint": null,
                    "visualHint": null,
                    "translationNote": null
                }
            ],
            "pageContext": null,
            "warnings": []
        }"#;
        let resp: UnlimitedOcrResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].id, "n1");
        assert_eq!(resp.items[0].text, "こんにちは");
        assert!(!resp.items[0].uncertain);
    }

    #[test]
    fn response_deserialize_with_context_fields() {
        let raw = r#"{
            "items": [{
                "id": "n1",
                "text": "test",
                "confidence": 0.92,
                "uncertain": false,
                "role": "narrator",
                "speakerHint": "narration",
                "emotionHint": "neutral",
                "visualHint": "close-up",
                "translationNote": "keep as-is"
            }],
            "pageContext": null,
            "warnings": []
        }"#;
        let resp: UnlimitedOcrResponse = serde_json::from_str(raw).unwrap();
        let item = &resp.items[0];
        assert_eq!(item.confidence, Some(0.92));
        assert_eq!(item.role.as_deref(), Some("narrator"));
        assert_eq!(item.speaker_hint.as_deref(), Some("narration"));
    }
}
