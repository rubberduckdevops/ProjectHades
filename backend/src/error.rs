use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

/// Every API error is rendered as `{"error": "<message>"}`.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    /// Details are logged but never returned to the client.
    #[error("internal error: {0}")]
    Internal(String),
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    pub fn internal(detail: impl std::fmt::Display) -> Self {
        Self::Internal(detail.to_string())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
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
            Self::BadRequest(m) | Self::Unauthorized(m) | Self::NotFound(m) => m.clone(),
        };
        (self.status(), Json(json!({ "error": message }))).into_response()
    }
}
