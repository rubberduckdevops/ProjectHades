mod auth;
mod config;
mod error;
mod handler;
mod models;
mod notes;
mod repo;

#[cfg(test)]
mod e2e_tests;

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
use tracing_subscriber::EnvFilter;

use crate::{
    auth::JwksVerifier,
    config::Config,
    handler::{journal, notes as notes_api},
};

pub struct AppState {
    pub table_name: String,
    pub ddb: aws_sdk_dynamodb::Client,
    pub s3: aws_sdk_s3::Client,
    pub notes_bucket: String,
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
        s3: aws_sdk_s3::Client::new(&aws_config),
        notes_bucket: config.notes_bucket.clone(),
        auth: Arc::new(JwksVerifier::new(
            &config.region,
            &config.user_pool_id,
            &config.client_id,
        )),
    });
    let app = build_app(state, &config.cors_origins);

    if on_lambda {
        tracing::info!(table = %config.table_name, bucket = %config.notes_bucket, "starting Lambda runtime");
        lambda_http::run(app).await
    } else {
        let addr = format!("0.0.0.0:{}", config.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::info!(table = %config.table_name, bucket = %config.notes_bucket, "listening on http://localhost:{}/api", config.port);
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

    /// DynamoDB and S3 clients pointing at a closed local port: any AWS call would fail with
    /// 500, so a 400/401 proves we rejected the request before touching AWS.
    fn test_app() -> Router {
        let ddb_conf = aws_sdk_dynamodb::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-west-2"))
            .credentials_provider(Credentials::new("test", "test", None, None, "test"))
            .endpoint_url("http://127.0.0.1:9")
            .build();
        let s3_conf = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-west-2"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "test", "test", None, None, "test",
            ))
            .endpoint_url("http://127.0.0.1:9")
            .force_path_style(true)
            .build();
        let state = Arc::new(AppState {
            table_name: "hades_journal_test".into(),
            ddb: aws_sdk_dynamodb::Client::from_conf(ddb_conf),
            s3: aws_sdk_s3::Client::from_conf(s3_conf),
            notes_bucket: "hades-notes-test".into(),
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
        let long_project = format!(r#"{{"body":"ok","project":"{}"}}"#, "p".repeat(101));
        for body in [
            r#"{"body":"   "}"#,
            r#"{"body":""}"#,
            too_big.as_str(),
            long_project.as_str(),
            r#"{"body":"ok","project":42}"#,
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

    // ---- Notes -----------------------------------------------------------------------------

    const HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn req(method: &str, uri: &str, body: Option<String>, auth: bool) -> Request<Body> {
        let mut b = Request::builder().method(method).uri(uri);
        if auth {
            b = b.header("authorization", bearer());
        }
        match body {
            Some(json) => b
                .header("content-type", "application/json")
                .body(Body::from(json))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        }
    }

    fn move_body(from: &str, to: &str) -> String {
        serde_json::json!({ "from": from, "to": to, "baseHash": HASH }).to_string()
    }

    async fn assert_400(r: Request<Body>) -> String {
        let label = format!("{} {}", r.method(), r.uri());
        let (status, _, body) = send(r).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}");
        error_message(&body)
    }

    #[tokio::test]
    async fn notes_require_token() {
        let put_body = Some(r#"{"content":"hi"}"#.to_owned());
        for r in [
            req("GET", "/api/notes", None, false),
            req("GET", "/api/notes?prefix=Projects/", None, false),
            req("GET", "/api/notes/a.md", None, false),
            req("PUT", "/api/notes/a.md", put_body, false),
            req(
                "DELETE",
                &format!("/api/notes/a.md?baseHash={HASH}"),
                None,
                false,
            ),
            req(
                "POST",
                "/api/notes/move",
                Some(move_body("a.md", "b.md")),
                false,
            ),
            // Auth is checked before the path is validated.
            req("GET", "/api/notes/a/../b.md", None, false),
        ] {
            let label = format!("{} {}", r.method(), r.uri());
            let (status, _, body) = send(r).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{label}");
            assert!(!error_message(&body).is_empty());
        }
    }

    #[tokio::test]
    async fn invalid_note_paths_are_400() {
        let put_body = || Some(r#"{"content":"hi"}"#.to_owned());
        for uri in [
            "/api/notes/a/../b.md",
            "/api/notes/a/%2E%2E/b.md",
            "/api/notes/./b.md",
            "/api/notes/noext",
            "/api/notes/a//b.md",
            "/api/notes/a.md/",
            "/api/notes/a%5Cb.md",
            "/api/notes/a%0Ab.md",
            "/api/notes/%20a.md",
            "/api/notes/a/%20/b.md",
        ] {
            assert!(!assert_400(req("GET", uri, None, true)).await.is_empty());
            assert_400(req("PUT", uri, put_body(), true)).await;
            assert_400(req("DELETE", &format!("{uri}?baseHash={HASH}"), None, true)).await;
        }
        let long = format!("/api/notes/{}.md", "a".repeat(510));
        assert_400(req("GET", &long, None, true)).await;
        // Invalid UTF-8 after percent-decoding.
        assert_400(req("GET", "/api/notes/%FF.md", None, true)).await;
    }

    #[tokio::test]
    async fn note_path_is_percent_decoded_exactly_once() {
        // `%2520` decodes once to the literal `%20`, which is a valid path, so the request
        // reaches storage (the fake endpoint -> 500). Decoding twice would yield `..` -> 400.
        let r = req("GET", "/api/notes/%252E%252E/b.md", None, true);
        let (status, _, body) = send(r).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error_message(&body), "internal server error");
        // Encoded spaces inside a segment are fine; leading space after decoding is not.
        let (status, _, _) = send(req("GET", "/api/notes/My%20Ideas/a.md", None, true)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_400(req("GET", "/api/notes/My%20Ideas/%20a.md", None, true)).await;
    }

    #[tokio::test]
    async fn put_note_bad_bodies_are_400() {
        let over = format!(
            r#"{{"content":"{}"}}"#,
            "a".repeat(crate::notes::MAX_CONTENT_BYTES + 1)
        );
        // Above axum's 2 MB default limit but within the route's raised limit.
        let three_mb = format!(r#"{{"content":"{}"}}"#, "a".repeat(3 * 1024 * 1024));
        let bad_hash = r#"{"content":"x","baseHash":"nothex"}"#.to_owned();
        for body in [
            over,
            three_mb,
            bad_hash,
            r#"{"content":42}"#.to_owned(),
            r#"{"nocontent":"x"}"#.to_owned(),
            "not json".to_owned(),
        ] {
            let msg = assert_400(req("PUT", "/api/notes/a.md", Some(body), true)).await;
            assert!(!msg.is_empty());
        }
        let msg = assert_400(req(
            "PUT",
            "/api/notes/a.md",
            Some(format!(
                r#"{{"content":"{}"}}"#,
                "a".repeat(crate::notes::MAX_CONTENT_BYTES + 1)
            )),
            true,
        ))
        .await;
        assert!(msg.contains("content must be at most"), "{msg}");
    }

    #[tokio::test]
    async fn oversized_request_body_is_json_error() {
        let huge = format!(
            r#"{{"content":"{}"}}"#,
            "a".repeat(notes_api::MAX_REQUEST_BYTES + 1)
        );
        let (status, _, body) = send(req("PUT", "/api/notes/a.md", Some(huge), true)).await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::PAYLOAD_TOO_LARGE,
            "{status}"
        );
        assert!(!error_message(&body).is_empty());
    }

    #[tokio::test]
    async fn delete_note_requires_valid_base_hash() {
        let msg = assert_400(req("DELETE", "/api/notes/a.md", None, true)).await;
        assert_eq!(msg, "baseHash is required");
        assert_400(req("DELETE", "/api/notes/a.md?baseHash=abc", None, true)).await;
    }

    #[tokio::test]
    async fn move_note_validation() {
        // Routed to `move` (not the wildcard, which has no POST handler and would give 405).
        let msg = assert_400(req(
            "POST",
            "/api/notes/move",
            Some(move_body("a.md", "a.md")),
            true,
        ))
        .await;
        assert_eq!(msg, "from and to must differ");
        for body in [
            move_body("a/../b.md", "c.md"),
            move_body("a.md", "noext"),
            move_body("/a.md", "b.md"),
            r#"{"from":"a.md","to":"b.md"}"#.to_owned(),
            serde_json::json!({ "from": "a.md", "to": "b.md", "baseHash": "x" }).to_string(),
            "not json".to_owned(),
        ] {
            assert_400(req("POST", "/api/notes/move", Some(body), true)).await;
        }
    }

    #[tokio::test]
    async fn valid_note_requests_reach_storage() {
        // With the fake endpoints, a valid request fails with a generic 500 (proving routing
        // and validation let it through).
        for r in [
            req("GET", "/api/notes", None, true),
            req("GET", "/api/notes?prefix=Projects/", None, true),
            req("GET", "/api/notes/Projects/Hades/Design.md", None, true),
            req(
                "PUT",
                "/api/notes/a.md",
                Some(r#"{"content":""}"#.into()),
                true,
            ),
            req(
                "DELETE",
                &format!("/api/notes/a.md?baseHash={HASH}"),
                None,
                true,
            ),
            req(
                "POST",
                "/api/notes/move",
                Some(move_body("a.md", "b.md")),
                true,
            ),
        ] {
            let label = format!("{} {}", r.method(), r.uri());
            let (status, _, body) = send(r).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{label}");
            assert_eq!(error_message(&body), "internal server error");
        }
    }

    #[tokio::test]
    async fn note_route_method_mismatches() {
        let (status, _, _) = send(req("POST", "/api/notes/a.md", None, true)).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        let (status, _, _) = send(req("GET", "/api/notes/move", None, true)).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn cors_preflight_allows_delete_and_post() {
        for method in ["DELETE", "POST"] {
            let r = Request::options("/api/notes/a.md")
                .header("origin", "http://localhost:5173")
                .header("access-control-request-method", method)
                .body(Body::empty())
                .unwrap();
            let (status, headers, _) = send(r).await;
            assert!(status.is_success());
            let allowed = headers["access-control-allow-methods"].to_str().unwrap();
            assert!(allowed.contains(method), "{allowed}");
        }
    }
}
