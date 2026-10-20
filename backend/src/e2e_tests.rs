//! End-to-end tests of the notes API, search and the indexer against real S3/DynamoDB wire
//! protocols, using a local mock such as moto (S3 Vectors and Bedrock are faked in-process).
//! Ignored by default so `cargo test` stays offline. Run with:
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
    indexer::{Change, Indexer, Item, Outcome, change_from_images},
    testing::{FakeEmbedder, FakeVectorStore},
};

struct Env {
    app: Router,
    s3: aws_sdk_s3::Client,
    bucket: String,
    ddb: aws_sdk_dynamodb::Client,
    table: String,
    vectors: Arc<FakeVectorStore>,
    indexer: Indexer,
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

    // S3 Vectors and Bedrock aren't available in moto: the API and the indexer share fakes.
    let embedder = Arc::new(FakeEmbedder::new());
    let vectors = Arc::new(FakeVectorStore::default());
    let state = Arc::new(AppState {
        table_name: table.clone(),
        ddb: ddb.clone(),
        s3: s3.clone(),
        notes_bucket: bucket.clone(),
        auth: Arc::new(static_verifier()),
        embedder: embedder.clone(),
        vectors: vectors.clone(),
    });
    let indexer = Indexer {
        ddb: ddb.clone(),
        s3: s3.clone(),
        table: table.clone(),
        bucket: bucket.clone(),
        embedder,
        vectors: vectors.clone(),
    };
    Env {
        app: build_app(state, &["http://localhost:5173".to_owned()]),
        s3,
        bucket,
        ddb,
        table,
        vectors,
        indexer,
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

// ---- Indexing + search (phase 2) -------------------------------------------------------------

/// The current metadata item (what a stream record's image would contain).
async fn item(env: &Env, sub: &str, path: &str) -> Option<Item> {
    use aws_sdk_dynamodb::types::AttributeValue;
    env.ddb
        .get_item()
        .table_name(&env.table)
        .key("pk", AttributeValue::S(format!("NOTE#{sub}")))
        .key("sk", AttributeValue::S(path.to_owned()))
        .consistent_read(true)
        .send()
        .await
        .unwrap()
        .item()
        .cloned()
}

/// Reconcile as the stream would for a record with these images.
async fn reconcile(env: &Env, event: &str, old: Option<&Item>, new: Option<&Item>) -> Outcome {
    let change = change_from_images(event, None, old, new).expect("valid record");
    env.indexer.apply(&change).await.expect("reconcile")
}

fn vector_count(env: &Env, sub: &str, path: &str) -> usize {
    env.vectors
        .keys_with_prefix(&format!("{sub}#{path}#"))
        .len()
}

fn section(name: &str, words: &str) -> String {
    format!("## {name}\n\n{}\n\n", vec![words; 40].join(" "))
}

#[tokio::test]
#[ignore = "needs HADES_E2E_ENDPOINT (local S3/DynamoDB mock)"]
async fn e2e_indexing_and_search() {
    let env = setup().await;
    let app = &env.app;
    let u = "user-1";
    let path = "Projects/Hades/Design.md";
    let uri = "/api/notes/Projects/Hades/Design.md";

    // Create: not indexed yet; INSERT reconcile indexes it.
    let v1 = format!(
        "---\nproject: hades\n---\n# Design\n\n{}{}{}",
        section("Storage", "content lives in versioned buckets"),
        section("Search", "vectors embeddings similarity"),
        section("Auth", "cognito tokens jwks")
    );
    let (s, created) = call(app, u, "PUT", uri, Some(json!({ "content": v1 }))).await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    assert_eq!(created["indexed"], false);
    let i1 = item(&env, u, path).await.unwrap();
    assert_eq!(
        reconcile(&env, "INSERT", None, Some(&i1)).await,
        Outcome::Indexed { chunks: 3 }
    );
    assert_eq!(vector_count(&env, u, path), 3);
    let (_, note) = call(app, u, "GET", uri, None).await;
    assert_eq!(note["indexed"], true);
    let meta = crate::vectors::HitMetadata::from_document(
        &env.vectors.metadata(&format!("{u}#{path}#0")).unwrap(),
    );
    assert_eq!(meta.project.as_deref(), Some("hades"));
    assert_eq!(meta.heading.as_deref(), Some("Design > Storage"));
    assert_eq!(meta.title.as_deref(), Some("Design"));

    // The indexer's own write-back record is a no-op.
    let indexed_item = item(&env, u, path).await.unwrap();
    assert_eq!(
        reconcile(&env, "MODIFY", Some(&i1), Some(&indexed_item)).await,
        Outcome::Skipped("already indexed")
    );

    // Semantic search finds it (neither title nor path contains the words).
    let (s, results) = call(app, u, "GET", "/api/search?q=vectors%20similarity", None).await;
    assert_eq!(s, StatusCode::OK, "{results}");
    assert_eq!(results[0]["path"], path);
    assert_eq!(results[0]["match"], "semantic");
    assert_eq!(results[0]["heading"], "Design > Search");
    // Another user finds nothing.
    let (_, other) = call(app, "user-2", "GET", "/api/search?q=vectors", None).await;
    assert_eq!(other, json!([]));
    // Project filter.
    let (_, r) = call(app, u, "GET", "/api/search?q=vectors&project=other", None).await;
    assert_eq!(r, json!([]));
    let (_, r) = call(app, u, "GET", "/api/search?q=vectors&project=hades", None).await;
    assert_eq!(r[0]["path"], path);

    // Update: indexer attributes survive the API's UpdateItem, so `indexed` flips to false.
    let h1 = created["contentHash"].as_str().unwrap().to_owned();
    let v2 = "# Design\n\nShort now.";
    let (s, updated) = call(
        app,
        u,
        "PUT",
        uri,
        Some(json!({ "content": v2, "baseHash": h1 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{updated}");
    assert_eq!(updated["indexed"], false);
    let i2 = item(&env, u, path).await.unwrap();
    assert_eq!(
        i2["indexedHash"].as_s().unwrap(),
        &h1,
        "indexedHash survives"
    );
    assert_eq!(i2["chunkCount"].as_n().unwrap(), "3", "chunkCount survives");
    assert_eq!(
        reconcile(&env, "MODIFY", Some(&indexed_item), Some(&i2)).await,
        Outcome::Indexed { chunks: 1 }
    );
    assert_eq!(vector_count(&env, u, path), 1, "stale chunks deleted");
    let (_, note) = call(app, u, "GET", uri, None).await;
    assert_eq!(note["indexed"], true);

    // Superseded: the note changes while an older record is being processed.
    let h2 = updated["contentHash"].as_str().unwrap().to_owned();
    let v3 = format!(
        "# Design\n\n{}{}",
        section("A", "alpha"),
        section("B", "beta")
    );
    let (_, n3) = call(
        app,
        u,
        "PUT",
        uri,
        Some(json!({ "content": v3, "baseHash": h2 })),
    )
    .await;
    let i3 = item(&env, u, path).await.unwrap();
    let h3 = n3["contentHash"].as_str().unwrap().to_owned();
    let (_, n4) = call(
        app,
        u,
        "PUT",
        uri,
        Some(json!({ "content": "# Design\n\nFinal.", "baseHash": h3 })),
    )
    .await;
    let i4 = item(&env, u, path).await.unwrap();
    assert_eq!(
        reconcile(&env, "MODIFY", None, Some(&i3)).await,
        Outcome::Superseded { chunks: 2 }
    );
    assert_eq!(
        vector_count(&env, u, path),
        1,
        "orphans beyond chunkCount cleaned up"
    );
    let (_, note) = call(app, u, "GET", uri, None).await;
    assert_eq!(
        note["indexed"], false,
        "superseded write-back must not mark indexed"
    );
    assert_eq!(
        reconcile(&env, "MODIFY", Some(&i3), Some(&i4)).await,
        Outcome::Indexed { chunks: 1 }
    );
    let (_, note) = call(app, u, "GET", uri, None).await;
    assert_eq!(note["indexed"], true);
    let h4 = n4["contentHash"].as_str().unwrap().to_owned();

    // Move: REMOVE for the old path, INSERT for the new one.
    let to = "Archive/Design.md";
    let (s, moved) = call(
        app,
        u,
        "POST",
        "/api/notes/move",
        Some(json!({ "from": path, "to": to, "baseHash": h4 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{moved}");
    assert_eq!(moved["indexed"], false);
    assert!(item(&env, u, path).await.is_none());
    let moved_item = item(&env, u, to).await.unwrap();
    assert!(
        !moved_item.contains_key("indexedHash"),
        "move writes a fresh item"
    );
    let before_move = item_after_index(&i4, &h4, 1);
    assert_eq!(
        reconcile(&env, "REMOVE", Some(&before_move), None).await,
        Outcome::Removed { vectors: 1 }
    );
    assert_eq!(
        reconcile(&env, "INSERT", None, Some(&moved_item)).await,
        Outcome::Indexed { chunks: 1 }
    );
    assert_eq!(vector_count(&env, u, path), 0);
    assert_eq!(vector_count(&env, u, to), 1);
    let (_, r) = call(app, u, "GET", "/api/search?q=final", None).await;
    assert_eq!(r[0]["path"], to);

    // Delete: REMOVE drops every vector.
    let deleted_item = item(&env, u, to).await.unwrap();
    let (s, _) = call(
        app,
        u,
        "DELETE",
        &format!("/api/notes/{to}?baseHash={h4}"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(
        reconcile(&env, "REMOVE", Some(&deleted_item), None).await,
        Outcome::Removed { vectors: 1 }
    );
    assert!(env.vectors.keys_with_prefix(&format!("{u}#")).is_empty());
    let (_, r) = call(app, u, "GET", "/api/search?q=final", None).await;
    assert_eq!(r, json!([]));

    // A stale record for a deleted note: write-back fails, and the vectors it wrote are removed.
    assert_eq!(
        reconcile(&env, "INSERT", None, Some(&moved_item)).await,
        Outcome::Superseded { chunks: 1 }
    );
    assert!(env.vectors.keys_with_prefix(&format!("{u}#")).is_empty());
}

/// `item` as the indexer's write-back would have left it.
fn item_after_index(item: &Item, hash: &str, chunks: u32) -> Item {
    use aws_sdk_dynamodb::types::AttributeValue;
    let mut i = item.clone();
    i.insert("indexedHash".into(), AttributeValue::S(hash.into()));
    i.insert("chunkCount".into(), AttributeValue::N(chunks.to_string()));
    i
}

#[tokio::test]
#[ignore = "needs HADES_E2E_ENDPOINT (local S3/DynamoDB mock)"]
async fn e2e_backfill() {
    let env = setup().await;
    let app = &env.app;
    for (user, path, content) in [
        ("user-1", "a.md", "# A\n\nalpha text"),
        ("user-1", "empty.md", ""),
        ("user-2", "b.md", "# B\n\nbeta text"),
    ] {
        let (s, _) = call(
            app,
            user,
            "PUT",
            &format!("/api/notes/{path}"),
            Some(json!({ "content": content })),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
    }
    let (s, _) = call(
        app,
        "user-1",
        "PUT",
        "/api/journal",
        Some(json!({ "body": "not a note" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    let report = env
        .indexer
        .handle(json!({ "backfill": true }))
        .await
        .unwrap();
    assert_eq!(
        report,
        json!({ "scanned": 3, "upToDate": 0, "reindexed": 3, "superseded": 0, "failed": 0 })
    );
    assert_eq!(vector_count(&env, "user-1", "a.md"), 1);
    assert_eq!(vector_count(&env, "user-1", "empty.md"), 0);
    assert_eq!(vector_count(&env, "user-2", "b.md"), 1);
    // An empty note has no vectors but still counts as indexed.
    let (_, notes) = call(app, "user-1", "GET", "/api/notes", None).await;
    assert!(
        notes
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["indexed"] == true),
        "{notes}"
    );

    let report = env
        .indexer
        .handle(json!({ "backfill": true }))
        .await
        .unwrap();
    assert_eq!(report["upToDate"], 3);
    assert_eq!(report["reindexed"], 0);

    // The stream path end to end: a record for an unindexed note.
    let (_, n) = call(
        app,
        "user-1",
        "PUT",
        "/api/notes/c.md",
        Some(json!({ "content": "gamma" })),
    )
    .await;
    assert_eq!(n["indexed"], false);
    let c = item(&env, "user-1", "c.md").await.unwrap();
    assert!(matches!(
        change_from_images("INSERT", None, None, Some(&c)).unwrap(),
        Change::Reindex { .. }
    ));
    let image: serde_json::Map<String, Value> = c
        .iter()
        .map(|(k, v)| {
            let av = match v {
                aws_sdk_dynamodb::types::AttributeValue::S(s) => json!({ "S": s }),
                aws_sdk_dynamodb::types::AttributeValue::N(n) => json!({ "N": n }),
                other => panic!("unexpected attribute {other:?}"),
            };
            (k.clone(), av)
        })
        .collect();
    let event = json!({ "Records": [{
        "eventID": "1", "eventName": "INSERT", "eventVersion": "1.1",
        "eventSource": "aws:dynamodb", "awsRegion": "us-west-2",
        "dynamodb": {
            "Keys": { "pk": image["pk"], "sk": image["sk"] },
            "NewImage": image, "SequenceNumber": "42", "SizeBytes": 100,
            "StreamViewType": "NEW_AND_OLD_IMAGES"
        }
    }]});
    let resp = env.indexer.handle(event).await.unwrap();
    assert_eq!(resp, json!({ "batchItemFailures": [] }));
    let (_, n) = call(app, "user-1", "GET", "/api/notes/c.md", None).await;
    assert_eq!(n["indexed"], true);
}

/// Regression: a backfill working from a stale scan snapshot must not leave old content in
/// the index after the stream has already indexed a newer version.
#[tokio::test]
#[ignore = "needs HADES_E2E_ENDPOINT (local S3/DynamoDB mock)"]
async fn e2e_backfill_stale_snapshot_does_not_win() {
    use crate::repo::notes::record_from_item;
    let env = setup().await;
    let app = &env.app;
    let (s, v1) = call(
        app,
        "user-1",
        "PUT",
        "/api/notes/race.md",
        Some(json!({ "content": "# Race\n\nzebra zebra old wording" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // The backfill's scan snapshot: version 1.
    let stale = record_from_item(&item(&env, "user-1", "race.md").await.unwrap()).unwrap();

    // The note is edited, and the stream indexes version 2 first.
    let (s, _) = call(app, "user-1", "PUT", "/api/notes/race.md",
        Some(json!({ "content": "# Race\n\nkoala koala new wording", "baseHash": v1["contentHash"] }))).await;
    assert_eq!(s, StatusCode::OK);
    let current = record_from_item(&item(&env, "user-1", "race.md").await.unwrap()).unwrap();
    let out = env.indexer.reindex("user-1", &current, 0).await.unwrap();
    assert!(matches!(out, Outcome::Indexed { .. }), "{out:?}");

    // Then the backfill processes its stale snapshot: its write-back loses...
    let out = env.indexer.reindex("user-1", &stale, 0).await.unwrap();
    assert!(matches!(out, Outcome::Superseded { .. }), "{out:?}");
    let snippet = |env: &Env| {
        let key = crate::vectors::vector_key("user-1", "race.md", 0);
        format!("{:?}", env.vectors.metadata(&key).expect("vector exists"))
    };
    assert!(
        snippet(&env).contains("zebra"),
        "the stale vectors did overwrite the new ones"
    );

    // ...and the backfill's recovery step restores the current version.
    let Outcome::Superseded { chunks } = out else {
        unreachable!()
    };
    env.indexer
        .reindex_current("user-1", "race.md", chunks)
        .await
        .unwrap();
    let s = snippet(&env);
    assert!(s.contains("koala") && !s.contains("zebra"), "{s}");
    let (_, note) = call(app, "user-1", "GET", "/api/notes/race.md", None).await;
    assert_eq!(note["indexed"], true);
}
