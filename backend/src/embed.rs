//! Text embeddings (see "Embeddings" in API_CONTRACT.md): Bedrock `InvokeModel` with Titan Text
//! Embeddings V2, 1024 dimensions, normalized.

use async_trait::async_trait;
use aws_sdk_bedrockruntime::{error::DisplayErrorContext, primitives::Blob};
use serde::Deserialize;
use serde_json::json;

use crate::error::ApiError;

/// Embedding dimension (must match the vector index).
pub const DIMENSIONS: usize = 1024;
/// Input is capped at this many characters before embedding.
pub const MAX_INPUT_CHARS: usize = 30_000;

/// Something that turns text into a vector.
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, ApiError>;
}

/// `text` truncated to at most `max` characters (on a char boundary).
pub fn cap_chars(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// The Titan V2 request body for `text` (capped at `MAX_INPUT_CHARS`).
pub fn request_body(text: &str) -> Vec<u8> {
    json!({
        "inputText": cap_chars(text, MAX_INPUT_CHARS),
        "dimensions": DIMENSIONS,
        "normalize": true,
    })
    .to_string()
    .into_bytes()
}

#[derive(Deserialize)]
struct TitanResponse {
    embedding: Vec<f32>,
}

/// Parse a Titan V2 response body and check the dimension.
pub fn parse_response(body: &[u8]) -> Result<Vec<f32>, ApiError> {
    let parsed: TitanResponse = serde_json::from_slice(body)
        .map_err(|e| ApiError::internal(format!("invalid embedding response: {e}")))?;
    if parsed.embedding.len() != DIMENSIONS {
        return Err(ApiError::internal(format!(
            "embedding has {} dimensions, expected {DIMENSIONS}",
            parsed.embedding.len()
        )));
    }
    Ok(parsed.embedding)
}

/// Bedrock Runtime `InvokeModel` embedder.
pub struct BedrockEmbedder {
    client: aws_sdk_bedrockruntime::Client,
    model_id: String,
}

impl BedrockEmbedder {
    pub fn new(client: aws_sdk_bedrockruntime::Client, model_id: impl Into<String>) -> Self {
        Self {
            client,
            model_id: model_id.into(),
        }
    }
}

#[async_trait]
impl Embedder for BedrockEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, ApiError> {
        let out = self
            .client
            .invoke_model()
            .model_id(&self.model_id)
            .content_type("application/json")
            .accept("application/json")
            .body(Blob::new(request_body(text)))
            .send()
            .await
            .map_err(|e| {
                ApiError::internal(format!("InvokeModel failed: {}", DisplayErrorContext(&e)))
            })?;
        parse_response(out.body().as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn body_per_contract() {
        let v: Value = serde_json::from_slice(&request_body("hello")).unwrap();
        assert_eq!(
            v,
            json!({ "inputText": "hello", "dimensions": 1024, "normalize": true })
        );
    }

    #[test]
    fn body_caps_input_on_char_boundary() {
        let long = "é".repeat(MAX_INPUT_CHARS + 5);
        let v: Value = serde_json::from_slice(&request_body(&long)).unwrap();
        let text = v["inputText"].as_str().unwrap();
        assert_eq!(text.chars().count(), MAX_INPUT_CHARS);
        assert_eq!(cap_chars("abc", 2), "ab");
        assert_eq!(cap_chars("abc", 5), "abc");
        assert_eq!(cap_chars("日本語", 1), "日");
    }

    #[test]
    fn parses_canned_response() {
        let emb: Vec<f32> = (0..DIMENSIONS).map(|i| i as f32 / 1000.0).collect();
        let body = json!({ "embedding": emb, "inputTextTokenCount": 3, "embeddingsByType": {} });
        let parsed = parse_response(body.to_string().as_bytes()).unwrap();
        assert_eq!(parsed.len(), DIMENSIONS);
        assert_eq!(parsed[1], 0.001);
    }

    #[test]
    fn rejects_bad_responses() {
        assert!(parse_response(b"not json").is_err());
        assert!(parse_response(br#"{"other": 1}"#).is_err());
        assert!(parse_response(br#"{"embedding": [0.1, 0.2]}"#).is_err());
    }
}
