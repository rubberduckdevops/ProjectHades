//! Router-level tests: routing, auth, validation and error mapping. AWS clients point at a
//! closed port, so any request that reaches DynamoDB/S3 fails with a generic 500.

use super::*;
use crate::{
    auth::test_support::{sign, static_verifier, valid_claims},
    testing::{FakeEmbedder, FakeVectorStore, aws_clients},
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

/// DynamoDB and S3 clients pointing at a closed local port: any AWS call would fail with
/// 500, so a 400/401 proves we rejected the request before touching AWS.
fn test_state(embedder: Arc<dyn Embedder>, vectors: Arc<dyn VectorStore>) -> Arc<AppState> {
    let (ddb, s3) = aws_clients("http://127.0.0.1:9");
    Arc::new(AppState {
        table_name: "hades_journal_test".into(),
        ddb,
        s3,
        notes_bucket: "hades-notes-test".into(),
        auth: Arc::new(static_verifier()),
        embedder,
        vectors,
    })
}

fn test_app() -> Router {
    let state = test_state(
        Arc::new(FakeEmbedder::new()),
        Arc::new(FakeVectorStore::default()),
    );
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
    let (status, _, body) = send(Request::get("/api/health").body(Body::empty()).unwrap()).await;
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
async fn cors_preflight_allows_delete_post_and_patch() {
    for method in ["DELETE", "POST", "PATCH"] {
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

// ---- Todos ---------------------------------------------------------------------------------

fn todo_req(method: &str, uri: &str, body: Option<&str>, auth: Option<String>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(a) = auth {
        b = b.header("authorization", a);
    }
    match body {
        Some(json) => b
            .header("content-type", "application/json")
            .body(Body::from(json.to_owned()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

const TODO_URI: &str = "/api/todos/2026-10-20T14:03:22.123Z";

#[tokio::test]
async fn todos_require_token() {
    for req in [
        todo_req("GET", "/api/todos", None, None),
        todo_req("POST", "/api/todos", Some(r#"{"title":"x"}"#), None),
        todo_req("PATCH", TODO_URI, Some(r#"{"done":true}"#), None),
        todo_req("DELETE", TODO_URI, None, None),
    ] {
        assert_eq!(send(req).await.0, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn todo_create_validation_is_400() {
    let long_title = format!(r#"{{"title":"{}"}}"#, "t".repeat(501));
    for body in [
        r#"{"title":"  "}"#,
        r#"{"nottitle":"x"}"#,
        long_title.as_str(),
        r#"{"title":"x","due":"next week"}"#,
        r#"{"title":"x","due":"2026-02-30"}"#,
        r#"{"title":"x","project":7}"#,
        "not json",
    ] {
        let (status, _, resp) =
            send(todo_req("POST", "/api/todos", Some(body), Some(bearer()))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body:.40}");
        assert!(!error_message(&resp).is_empty());
    }
}

#[tokio::test]
async fn todo_patch_and_delete_validation_is_400() {
    for (method, uri, body) in [
        ("PATCH", "/api/todos/not-a-time", Some(r#"{"done":true}"#)),
        ("DELETE", "/api/todos/not-a-time", None),
        ("PATCH", TODO_URI, Some(r#"{"title":""}"#)),
        ("PATCH", TODO_URI, Some(r#"{"due":"soon"}"#)),
        ("PATCH", TODO_URI, Some(r#"{"done":"yes"}"#)),
    ] {
        let (status, _, _) = send(todo_req(method, uri, body, Some(bearer()))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri} {body:?}");
    }
}

#[tokio::test]
async fn todo_valid_requests_reach_dynamodb() {
    // The DynamoDB client points at a closed port, so a 500 proves validation passed.
    for (method, uri, body) in [
        ("GET", "/api/todos", None),
        (
            "POST",
            "/api/todos",
            Some(r#"{"title":"x","project":"p","due":"2026-11-01"}"#),
        ),
        (
            "PATCH",
            TODO_URI,
            Some(r#"{"project":null,"due":null,"done":true}"#),
        ),
        ("DELETE", TODO_URI, None),
    ] {
        let (status, _, _) = send(todo_req(method, uri, body, Some(bearer()))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{method} {uri}");
    }
}

// ---- Projects ------------------------------------------------------------------------------

#[tokio::test]
async fn projects_require_token() {
    for req in [
        todo_req("GET", "/api/projects", None, None),
        todo_req("DELETE", "/api/projects/hades", None, None),
    ] {
        assert_eq!(send(req).await.0, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn project_delete_validation_is_400() {
    let long = format!("/api/projects/{}", "p".repeat(101));
    for uri in ["/api/projects/%20%20", long.as_str()] {
        let (status, _, _) = send(todo_req("DELETE", uri, None, Some(bearer()))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri:.40}");
    }
}

#[tokio::test]
async fn project_valid_requests_reach_dynamodb() {
    for (method, uri) in [
        ("GET", "/api/projects"),
        ("DELETE", "/api/projects/My%20Project"),
    ] {
        let (status, _, _) = send(todo_req(method, uri, None, Some(bearer()))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{method} {uri}");
    }
}

// ---- Search --------------------------------------------------------------------------------

mod search {
    use super::*;
    use crate::{
        testing::{FailingEmbedder, bag_of_words, fake_dynamodb, note_item_json},
        vectors::{ChunkMetadata, NewVector, doc_to_json, vector_key},
    };
    use serde_json::json;

    fn chunk(sub: &str, path: &str, n: u32, project: Option<&str>, text: &str) -> NewVector {
        NewVector {
            key: vector_key(sub, path, n),
            data: bag_of_words(text),
            metadata: ChunkMetadata {
                sub: sub.into(),
                path: path.into(),
                project: project.map(str::to_owned),
                heading: format!("{path} heading {n}"),
                snippet: format!("{sub}: {text}"),
                title: "stale title".into(),
                updated_at: "stale".into(),
            }
            .to_document(),
        }
    }

    /// user-1 has four notes; user-2 has vectors that match the query perfectly, including one
    /// at the same path as a user-1 note.
    async fn fixture() -> (Router, Arc<FakeVectorStore>) {
        let notes = vec![
            note_item_json(
                "user-1",
                "Projects/Hades/Design.md",
                "Design",
                Some("hades"),
                "2026-10-01T00:00:00.000Z",
            ),
            note_item_json(
                "user-1",
                "Inbox/storage-notes.md",
                "Misc",
                None,
                "2026-10-05T00:00:00.000Z",
            ),
            note_item_json(
                "user-1",
                "Storage.md",
                "Storage Plan",
                None,
                "2026-10-02T00:00:00.000Z",
            ),
            note_item_json(
                "user-1",
                "cake.md",
                "Cake",
                None,
                "2026-10-03T00:00:00.000Z",
            ),
            note_item_json(
                "user-2",
                "Secret.md",
                "Storage secrets",
                None,
                "2026-10-09T00:00:00.000Z",
            ),
        ];
        let store = Arc::new(FakeVectorStore::default());
        store
            .put(vec![
                chunk(
                    "user-1",
                    "Projects/Hades/Design.md",
                    0,
                    Some("hades"),
                    "intro to the design",
                ),
                chunk(
                    "user-1",
                    "Projects/Hades/Design.md",
                    1,
                    Some("hades"),
                    "content lives in s3 storage buckets",
                ),
                chunk("user-1", "cake.md", 0, None, "chocolate cake recipe"),
                // Indexed but since deleted: no metadata item.
                chunk("user-1", "Gone.md", 0, None, "storage storage"),
                chunk("user-2", "Secret.md", 0, None, "storage"),
                chunk("user-2", "cake.md", 0, None, "storage"),
            ])
            .await
            .unwrap();
        let endpoint = fake_dynamodb(notes).await;
        let (ddb, s3) = aws_clients(&endpoint);
        let state = Arc::new(AppState {
            table_name: "t".into(),
            ddb,
            s3,
            notes_bucket: "b".into(),
            auth: Arc::new(static_verifier()),
            embedder: Arc::new(FakeEmbedder::new()),
            vectors: store.clone(),
        });
        (build_app(state, &[]), store)
    }

    async fn get_json(app: &Router, uri: &str, user: &str) -> (StatusCode, Value) {
        let r = Request::get(uri)
            .header(
                "authorization",
                format!("Bearer {}", sign(&valid_claims(user))),
            )
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(r).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    fn paths(v: &Value) -> Vec<&str> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|r| r["path"].as_str().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn requires_token() {
        let (status, _, body) = send(req("GET", "/api/search?q=x", None, false)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!error_message(&body).is_empty());
    }

    #[tokio::test]
    async fn bad_params_are_400() {
        let long = format!("/api/search?q={}", "a".repeat(501));
        for uri in [
            "/api/search",
            "/api/search?q=",
            "/api/search?q=%20%20",
            long.as_str(),
            "/api/search?q=x&limit=0",
            "/api/search?q=x&limit=51",
            "/api/search?q=x&limit=ten",
            "/api/search?q=x&limit=",
        ] {
            assert!(
                !assert_400(req("GET", uri, None, true)).await.is_empty(),
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn ranks_name_matches_first_then_semantic() {
        let (app, store) = fixture().await;
        let (status, body) = get_json(&app, "/api/search?q=Storage", "user-1").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            paths(&body),
            [
                "Storage.md",             // title match
                "Inbox/storage-notes.md", // path-only match
                "Projects/Hades/Design.md",
                "cake.md",
            ]
        );
        assert_eq!(body[0]["match"], "name");
        assert_eq!(body[0]["score"], Value::Null);
        assert_eq!(body[0]["heading"], Value::Null);
        assert_eq!(body[0]["snippet"], Value::Null);
        // Semantic hit: best chunk wins; title/project/updatedAt come from current metadata.
        let design = &body[2];
        assert_eq!(design["match"], "semantic");
        assert_eq!(design["title"], "Design");
        assert_eq!(design["project"], "hades");
        assert_eq!(design["updatedAt"], "2026-10-01T00:00:00.000Z");
        assert_eq!(design["heading"], "Projects/Hades/Design.md heading 1");
        assert_eq!(
            design["snippet"],
            "user-1: content lives in s3 storage buckets"
        );
        let score = design["score"].as_f64().unwrap();
        assert!(score > 0.0 && score <= 1.0, "{score}");
        // The filter always carries the caller's sub.
        let filters = store.filters.lock().unwrap();
        assert_eq!(
            doc_to_json(filters.last().unwrap()),
            json!({ "sub": { "$eq": "user-1" } })
        );
    }

    #[tokio::test]
    async fn other_users_vectors_never_leak() {
        let (app, _) = fixture().await;
        let (_, body) = get_json(&app, "/api/search?q=storage", "user-1").await;
        let all = body.to_string();
        assert!(!all.contains("Secret.md"), "{all}");
        assert!(!all.contains("user-2"), "{all}");
        let cake = body
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == "cake.md")
            .unwrap();
        assert_eq!(cake["snippet"], "user-1: chocolate cake recipe");
        // Deleted notes are dropped even with a strong vector match.
        assert!(!all.contains("Gone.md"));

        // user-2 only sees their own note.
        let (_, body) = get_json(&app, "/api/search?q=storage", "user-2").await;
        assert_eq!(paths(&body), ["Secret.md"]);
        // A user with no notes and no vectors gets nothing.
        let (status, body) = get_json(&app, "/api/search?q=storage", "user-3").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!([]));
    }

    #[tokio::test]
    async fn project_filter_and_limit() {
        let (app, store) = fixture().await;
        let (_, body) = get_json(&app, "/api/search?q=storage&project=hades", "user-1").await;
        assert_eq!(paths(&body), ["Projects/Hades/Design.md"]);
        assert_eq!(
            doc_to_json(store.filters.lock().unwrap().last().unwrap()),
            json!({ "$and": [
                { "sub": { "$eq": "user-1" } },
                { "project": { "$eq": "hades" } }
            ] })
        );
        let (_, body) = get_json(&app, "/api/search?q=storage&limit=1", "user-1").await;
        assert_eq!(paths(&body), ["Storage.md"]);
        // A name match that also has a semantic hit appears once, with its snippet.
        let (_, body) = get_json(&app, "/api/search?q=design", "user-1").await;
        assert_eq!(paths(&body)[0], "Projects/Hades/Design.md");
        assert_eq!(body[0]["match"], "name");
        assert!(body[0]["snippet"].as_str().unwrap().starts_with("user-1: "));
        assert_eq!(
            paths(&body)
                .iter()
                .filter(|p| **p == "Projects/Hades/Design.md")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn embedding_failure_is_500() {
        let state = test_state(
            Arc::new(FailingEmbedder),
            Arc::new(FakeVectorStore::default()),
        );
        let app = build_app(state, &[]);
        let (status, body) = get_json(&app, "/api/search?q=x", "user-1").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "internal server error");
    }
}
