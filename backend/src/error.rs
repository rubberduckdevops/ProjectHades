use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

/// Every API error is rendered as `{"error": "<message>"}`. A `Conflict` additionally carries
/// `currentHash` when the note exists.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    /// 409. `current_hash` is the stored `contentHash` of the note, when it exists.
    #[error("{message}")]
    Conflict {
        message: String,
        current_hash: Option<String>,
    },
    /// Details are logged but never returned to the client.
    #[error("internal error: {0}")]
    Internal(String),
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    pub fn conflict(msg: impl Into<String>, current_hash: Option<String>) -> Self {
        Self::Conflict {
            message: msg.into(),
            current_hash,
        }
    }

    pub fn internal(detail: impl std::fmt::Display) -> Self {
        Self::Internal(detail.to_string())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let message = match &self {
            Self::Internal(detail) => {
                tracing::error!(error = %detail, "internal server error");
                "internal server error".to_owned()
            }
            Self::Conflict {
                message,
                current_hash: Some(hash),
            } => {
                return (
                    self.status(),
                    Json(json!({ "error": message, "currentHash": hash })),
                )
                    .into_response();
            }
            Self::BadRequest(m)
            | Self::Unauthorized(m)
            | Self::NotFound(m)
            | Self::Conflict { message: m, .. } => m.clone(),
        };
        (self.status(), Json(json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use serde_json::Value;

    async fn render(e: ApiError) -> (StatusCode, Value) {
        let res = e.into_response();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn conflict_with_hash() {
        let (status, body) =
            render(ApiError::conflict("note was changed", Some("ab".into()))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body,
            json!({ "error": "note was changed", "currentHash": "ab" })
        );
    }

    #[tokio::test]
    async fn conflict_without_hash_omits_field() {
        let (status, body) = render(ApiError::conflict("target exists", None)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body, json!({ "error": "target exists" }));
    }

    #[tokio::test]
    async fn internal_hides_detail() {
        let (status, body) = render(ApiError::internal("secret detail")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body, json!({ "error": "internal server error" }));
    }
}
