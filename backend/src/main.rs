mod auth;
mod config;
mod error;
mod handler;
mod models;
mod repo;

use std::sync::Arc;

use axum::{
    Router,
    http::{HeaderValue, Method, header},
    routing::get,
};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

use crate::{auth::JwksVerifier, config::Config, handler::journal};

pub struct AppState {
    pub table_name: String,
    pub ddb: aws_sdk_dynamodb::Client,
    pub auth: Arc<JwksVerifier>,
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
        .allow_methods([Method::GET, Method::PUT, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

fn build_app(state: Arc<AppState>, cors_origins: &[String]) -> Router {
    build_default_router(state).layer(cors_layer(cors_origins))
}

#[tokio::main]
async fn main() -> Result<(), lambda_http::Error> {
    let on_lambda = std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .with_ansi(!on_lambda)
        .without_time()
        .init();

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(1);
        }
    };

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(config.region.clone()))
        .load()
        .await;
    let state = Arc::new(AppState {
        table_name: config.table_name.clone(),
        ddb: aws_sdk_dynamodb::Client::new(&aws_config),
        auth: Arc::new(JwksVerifier::new(
            &config.region,
            &config.user_pool_id,
            &config.client_id,
        )),
    });
    let app = build_app(state, &config.cors_origins);

    if on_lambda {
        tracing::info!(table = %config.table_name, "starting Lambda runtime");
        lambda_http::run(app).await
    } else {
        let addr = format!("0.0.0.0:{}", config.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::info!(table = %config.table_name, "listening on http://localhost:{}/api", config.port);
        axum::serve(listener, app).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::{sign, static_verifier, valid_claims};
    use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    /// DynamoDB client pointing at a closed local port: any DDB call would fail with 500,
    /// so a 400/401 proves we rejected the request before touching DynamoDB.
    fn test_app() -> Router {
        let ddb_conf = aws_sdk_dynamodb::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-west-2"))
            .credentials_provider(Credentials::new("test", "test", None, None, "test"))
            .endpoint_url("http://127.0.0.1:9")
            .build();
        let state = Arc::new(AppState {
            table_name: "hades_journal_test".into(),
            ddb: aws_sdk_dynamodb::Client::from_conf(ddb_conf),
            auth: Arc::new(static_verifier()),
        });
        build_app(state, &["http://localhost:5173".to_owned()])
    }

    fn bearer() -> String {
        format!("Bearer {}", sign(&valid_claims("user-1")))
    }

    async fn send(req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let res = test_app().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, headers, body)
    }

    fn error_message(body: &[u8]) -> String {
        let v: Value = serde_json::from_slice(body).expect("error body is JSON");
        v["error"].as_str().expect("has error field").to_owned()
    }

    fn put(body: &str, auth: Option<String>) -> Request<Body> {
        let mut b = Request::put("/api/journal").header("content-type", "application/json");
        if let Some(a) = auth {
            b = b.header("authorization", a);
        }
        b.body(Body::from(body.to_owned())).unwrap()
    }

    #[tokio::test]
    async fn health_is_ok_without_auth() {
        let (status, _, body) =
            send(Request::get("/api/health").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"ok");
    }

    #[tokio::test]
    async fn unknown_route_is_404() {
        let (status, _, _) = send(Request::get("/journal").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn journal_requires_token() {
        for req in [
            Request::get("/api/journal").body(Body::empty()).unwrap(),
            Request::get("/api/journal/2026-10-19T14:03:22.123Z")
                .body(Body::empty())
                .unwrap(),
            put(r#"{"body":"hi"}"#, None),
        ] {
            let (status, _, body) = send(req).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(!error_message(&body).is_empty());
        }
    }

    #[tokio::test]
    async fn invalid_token_is_401() {
        let mut claims = valid_claims("u");
        claims["token_use"] = "id".into();
        let req = Request::get("/api/journal")
            .header("authorization", format!("Bearer {}", sign(&claims)))
            .body(Body::empty())
            .unwrap();
        let (status, _, body) = send(req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error_message(&body), "invalid token");

        let req = Request::get("/api/journal")
            .header("authorization", "Basic abc")
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(req).await.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn put_with_bad_body_is_400() {
        let too_big = format!(r#"{{"body":"{}"}}"#, "a".repeat(10 * 1024 + 1));
        for body in [
            r#"{"body":"   "}"#,
            r#"{"body":""}"#,
            too_big.as_str(),
            r#"{"body":"ok","id":"not-a-time"}"#,
            r#"{"nobody":"x"}"#,
            "not json",
        ] {
            let (status, _, resp) = send(put(body, Some(bearer()))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body:.40}");
            assert!(!error_message(&resp).is_empty());
        }
    }

    #[tokio::test]
    async fn bad_timestamps_are_400() {
        for uri in [
            "/api/journal/not-a-time",
            "/api/journal?from=yesterday",
            "/api/journal?to=2026-10-19",
            "/api/journal?from=2026-10-20T00:00:00Z&to=2026-10-19T00:00:00Z",
        ] {
            let req = Request::get(uri)
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap();
            let (status, _, _) = send(req).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        }
    }

    #[tokio::test]
    async fn ddb_failure_is_generic_500() {
        let req = Request::get("/api/journal")
            .header("authorization", bearer())
            .body(Body::empty())
            .unwrap();
        let (status, _, body) = send(req).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error_message(&body), "internal server error");
    }

    #[tokio::test]
    async fn cors_preflight_allows_configured_origin() {
        let req = Request::options("/api/journal")
            .header("origin", "http://localhost:5173")
            .header("access-control-request-method", "PUT")
            .header(
                "access-control-request-headers",
                "authorization,content-type",
            )
            .body(Body::empty())
            .unwrap();
        let (status, headers, _) = send(req).await;
        assert!(status.is_success());
        assert_eq!(
            headers["access-control-allow-origin"],
            "http://localhost:5173"
        );
    }
}
