//! Hades backend library: shared by the API binary (`src/main.rs`) and the search indexer
//! (`src/bin/indexer.rs`). See API_CONTRACT.md.

pub mod auth;
pub mod chunker;
pub mod config;
pub mod embed;
pub mod error;
pub mod handler;
pub mod indexer;
pub mod models;
pub mod notes;
pub mod repo;
pub mod search;
pub mod vectors;

#[cfg(test)]
mod e2e_tests;
#[cfg(test)]
mod router_tests;
#[cfg(test)]
pub(crate) mod testing;

use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderValue, Method, header},
    routing::{get, post},
};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};

use crate::{
    auth::JwksVerifier,
    embed::Embedder,
    handler::{journal, notes as notes_api, search as search_api},
    vectors::VectorStore,
};

pub struct AppState {
    pub table_name: String,
    pub ddb: aws_sdk_dynamodb::Client,
    pub s3: aws_sdk_s3::Client,
    pub notes_bucket: String,
    pub auth: Arc<JwksVerifier>,
    /// Text embeddings (Bedrock in production, a deterministic fake in tests).
    pub embedder: Arc<dyn Embedder>,
    /// The notes vector index (S3 Vectors in production, in-memory in tests).
    pub vectors: Arc<dyn VectorStore>,
}

/// API routes (mounted under `/api`), without CORS.
fn build_default_router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(
            "/journal",
            get(journal::list_journal_entries).put(journal::put_journal_entry),
        )
        .route("/journal/{timestamp}", get(journal::get_journal_entry))
        .route("/notes", get(notes_api::list_notes))
        // Static route; takes precedence over the wildcard below (note paths end in `.md`).
        .route("/notes/move", post(notes_api::move_note))
        .route(
            "/notes/{*path}",
            get(notes_api::get_note)
                .put(notes_api::put_note)
                .delete(notes_api::delete_note)
                .layer(DefaultBodyLimit::max(notes_api::MAX_REQUEST_BYTES)),
        )
        .route("/search", get(search_api::search))
        .with_state(state);

    Router::new()
        .nest("/api", api)
        .layer(TraceLayer::new_for_http())
}

fn cors_layer(origins: &[String]) -> CorsLayer {
    let allow_origin = if origins.iter().any(|o| o == "*") {
        AllowOrigin::any()
    } else {
        AllowOrigin::list(
            origins
                .iter()
                .filter_map(|o| match HeaderValue::from_str(o) {
                    Ok(v) => Some(v),
                    Err(_) => {
                        tracing::warn!(origin = %o, "ignoring invalid CORS origin");
                        None
                    }
                }),
        )
    };
    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([
            Method::GET,
            Method::PUT,
            Method::POST,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

/// The full application: API routes plus CORS.
pub fn build_app(state: Arc<AppState>, cors_origins: &[String]) -> Router {
    build_default_router(state).layer(cors_layer(cors_origins))
}

/// Initialize `tracing` (plain output on Lambda, colored locally).
pub fn init_tracing(on_lambda: bool) {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,tower_http=info")),
        )
        .with_ansi(!on_lambda)
        .without_time()
        .init();
}
