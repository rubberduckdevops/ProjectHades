//! End-to-end tests of the notes API against real S3/DynamoDB wire protocols, using a local
//! mock such as moto. Ignored by default so `cargo test` stays offline. Run with:
//!
//! ```sh
//! uvx --from 'moto[server]' moto_server -p 5055 &
//! HADES_E2E_ENDPOINT=http://127.0.0.1:5055 cargo test e2e -- --ignored --test-threads=1
//! ```

use std::sync::Arc;

use aws_sdk_dynamodb::{
    config::{BehaviorVersion, Credentials, Region},
    types::{AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType},
};
use aws_sdk_s3::types::{BucketVersioningStatus, VersioningConfiguration};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::{
    AppState,
    auth::test_support::{sign, static_verifier, valid_claims},
    build_app,
};

struct Env {
    app: Router,
    s3: aws_sdk_s3::Client,
    bucket: String,
}

async fn setup() -> Env {
    let endpoint = std::env::var("HADES_E2E_ENDPOINT")
        .expect("set HADES_E2E_ENDPOINT to a local S3/DynamoDB mock, e.g. moto");
    // Unique names per run so reruns against the same mock don't collide.
    let run = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let table = format!("hades-e2e-{run}");
    let bucket = format!("hades-e2e-{run}");

    let ddb = aws_sdk_dynamodb::Client::from_conf(
        aws_sdk_dynamodb::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-west-2"))
            .credentials_provider(Credentials::new("test", "test", None, None, "test"))
            .endpoint_url(&endpoint)
            .build(),
    );
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-west-2"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "test", "test", None, None, "test",
            ))
            .endpoint_url(&endpoint)
            .force_path_style(true)
            .build(),
    );

    ddb.create_table()
        .table_name(&table)
        .billing_mode(BillingMode::PayPerRequest)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("pk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("sk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("pk")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("sk")
                .key_type(KeyType::Range)
                .build()
                .unwrap(),
        )
        .send()
        .await
        .expect("create table");
    s3.create_bucket()
        .bucket(&bucket)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::UsWest2)
                .build(),
        )
        .send()
        .await
        .expect("create bucket");
    s3.put_bucket_versioning()
        .bucket(&bucket)
        .versioning_configuration(
            VersioningConfiguration::builder()
                .status(BucketVersioningStatus::Enabled)
                .build(),
        )
        .send()
        .await
        .expect("enable versioning");

    let state = Arc::new(AppState {
        table_name: table,
        ddb,
        s3: s3.clone(),
        notes_bucket: bucket.clone(),
        auth: Arc::new(static_verifier()),
    });
    Env {
        app: build_app(state, &["http://localhost:5173".to_owned()]),
        s3,
        bucket,
    }
}

async fn call(
    app: &Router,
    user: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri).header(
        "authorization",
        format!("Bearer {}", sign(&valid_claims(user))),
    );
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("JSON body")
    };
    (status, json)
}

#[tokio::test]
#[ignore = "needs HADES_E2E_ENDPOINT (local S3/DynamoDB mock)"]
async fn e2e_notes_lifecycle() {
    let env = setup().await;
    let app = &env.app;
    let u = "user-1";
    let path = "/api/notes/Projects/My%20Ideas/Design%20%231.md";
    let real_path = "Projects/My Ideas/Design #1.md";

    // Create.
    let content = "---\nproject: hades\ntags: [arch, \"#notes\"]\n---\n# Design\n\nFirst draft.";
    let (s, created) = call(app, u, "PUT", path, Some(json!({ "content": content }))).await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    assert_eq!(created["path"], real_path);
    assert_eq!(created["title"], "Design");
    assert_eq!(created["project"], "hades");
    assert_eq!(created["tags"], json!(["arch", "notes"]));
    let h1 = created["contentHash"].as_str().unwrap().to_owned();
    assert_eq!(h1.len(), 64);

    // Create again -> 409 with currentHash.
    let (s, body) = call(app, u, "PUT", path, Some(json!({ "content": "other" }))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(body["currentHash"], h1.as_str());

    // Read back exact content.
    let (s, note) = call(app, u, "GET", path, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(note["content"], content);
    assert_eq!(note["contentHash"], h1.as_str());

    // Update with the right baseHash; project removed, title from H1.
    let v2 = "# Design v2\n\nSecond draft.";
    let (s, updated) = call(
        app,
        u,
        "PUT",
        path,
        Some(json!({ "content": v2, "baseHash": h1 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{updated}");
    let h2 = updated["contentHash"].as_str().unwrap().to_owned();
    assert_ne!(h1, h2);
    assert_eq!(updated["title"], "Design v2");
    assert_eq!(updated["project"], Value::Null);
    assert_eq!(updated["tags"], json!([]));
    assert_eq!(
        updated["createdAt"], created["createdAt"],
        "createdAt is kept"
    );

    // Stale update -> 409 with the current hash, and the stored note is unchanged.
    let (s, body) = call(
        app,
        u,
        "PUT",
        path,
        Some(json!({ "content": "stale edit", "baseHash": h1 })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(body["currentHash"], h2.as_str());
    let (_, note) = call(app, u, "GET", path, None).await;
    assert_eq!(note["content"], v2, "rolled-back write must not be visible");

    // No-op update (same content) -> 200, no new version.
    let versions_before = count_versions(&env).await;
    let (s, same) = call(
        app,
        u,
        "PUT",
        path,
        Some(json!({ "content": v2, "baseHash": h2 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(same["contentHash"], h2.as_str());
    assert_eq!(
        count_versions(&env).await,
        versions_before,
        "no-op must not write to S3"
    );

    // Update a missing note -> 404.
    let (s, _) = call(
        app,
        u,
        "PUT",
        "/api/notes/missing.md",
        Some(json!({ "content": "x", "baseHash": h2 })),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Another user cannot see or touch it.
    let (s, _) = call(app, "user-2", "GET", path, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, list2) = call(app, "user-2", "GET", "/api/notes", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list2, json!([]));
    let (s, _) = call(
        app,
        "user-2",
        "DELETE",
        &format!("{path}?baseHash={h2}"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // List + prefix.
    let (s, _) = call(
        app,
        u,
        "PUT",
        "/api/notes/Inbox/todo.md",
        Some(json!({ "content": "- [ ] thing" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, all) = call(app, u, "GET", "/api/notes", None).await;
    let paths: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["Inbox/todo.md", real_path], "sorted by path");
    let (_, projects) = call(app, u, "GET", "/api/notes?prefix=Projects%2F", None).await;
    assert_eq!(projects.as_array().unwrap().len(), 1);

    // Move: stale hash -> 409 with currentHash; onto existing -> 409 without; then success.
    let (s, body) = call(
        app,
        u,
        "POST",
        "/api/notes/move",
        Some(json!({ "from": real_path, "to": "Archive/Design.md", "baseHash": h1 })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(body["currentHash"], h2.as_str());
    let (s, body) = call(
        app,
        u,
        "POST",
        "/api/notes/move",
        Some(json!({ "from": real_path, "to": "Inbox/todo.md", "baseHash": h2 })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "target exists");
    assert!(
        body.get("currentHash").is_none(),
        "target-exists must not carry currentHash"
    );
    let (_, todo) = call(app, u, "GET", "/api/notes/Inbox/todo.md", None).await;
    assert_eq!(
        todo["content"], "- [ ] thing",
        "failed move must not clobber the target"
    );

    let (s, moved) = call(
        app,
        u,
        "POST",
        "/api/notes/move",
        Some(json!({ "from": real_path, "to": "Archive/Design.md", "baseHash": h2 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{moved}");
    assert_eq!(moved["path"], "Archive/Design.md");
    assert_eq!(moved["contentHash"], h2.as_str());
    assert_eq!(moved["createdAt"], created["createdAt"]);
    let (s, _) = call(app, u, "GET", path, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "old path is gone");
    let (_, at_new) = call(app, u, "GET", "/api/notes/Archive/Design.md", None).await;
    assert_eq!(at_new["content"], v2);

    // Delete: missing baseHash -> 400, stale -> 409, right -> 204 (empty), then 404.
    let (s, _) = call(app, u, "DELETE", "/api/notes/Archive/Design.md", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, body) = call(
        app,
        u,
        "DELETE",
        &format!("/api/notes/Archive/Design.md?baseHash={h1}"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(body["currentHash"], h2.as_str());
    let (s, body) = call(
        app,
        u,
        "DELETE",
        &format!("/api/notes/Archive/Design.md?baseHash={h2}"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null, "204 has no body");
    let (s, _) = call(app, u, "GET", "/api/notes/Archive/Design.md", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // History is kept: the deleted note's versions still exist behind a delete marker.
    let out = env
        .s3
        .list_object_versions()
        .bucket(&env.bucket)
        .prefix("users/user-1/notes/Archive/Design.md")
        .send()
        .await
        .unwrap();
    assert!(!out.versions().is_empty(), "versions kept");
    assert!(!out.delete_markers().is_empty(), "delete marker present");

    // Recreate at a deleted path works.
    let (s, _) = call(
        app,
        u,
        "PUT",
        "/api/notes/Archive/Design.md",
        Some(json!({ "content": "again" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
}

#[tokio::test]
#[ignore = "needs HADES_E2E_ENDPOINT (local S3/DynamoDB mock)"]
async fn e2e_journal_still_works() {
    let env = setup().await;
    let (s, entry) = call(
        &env.app,
        "user-1",
        "PUT",
        "/api/journal",
        Some(json!({ "body": "hello", "project": "hades" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{entry}");
    let (s, list) = call(&env.app, "user-1", "GET", "/api/journal", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["project"], "hades");
    // Notes items in the same table don't leak into the journal and vice versa.
    let (_, notes) = call(&env.app, "user-1", "GET", "/api/notes", None).await;
    assert_eq!(notes, json!([]));
}

async fn count_versions(env: &Env) -> usize {
    env.s3
        .list_object_versions()
        .bucket(&env.bucket)
        .send()
        .await
        .unwrap()
        .versions()
        .len()
}
