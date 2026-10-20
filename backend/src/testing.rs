//! Test doubles: a deterministic embedder and an in-memory vector store that evaluates the
//! `$eq` / `$and` filter subset the API uses.

use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use aws_smithy_types::Document;

use crate::{
    embed::{DIMENSIONS, Embedder},
    error::ApiError,
    vectors::{NewVector, VectorMatch, VectorStore},
};

/// Hashed bag-of-words embedding, L2-normalized. Texts sharing words get similar vectors.
pub struct FakeEmbedder {
    pub calls: AtomicUsize,
}

impl FakeEmbedder {
    pub fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

pub fn bag_of_words(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; DIMENSIONS];
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let mut h = DefaultHasher::new();
        word.to_lowercase().hash(&mut h);
        v[(h.finish() % DIMENSIONS as u64) as usize] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

#[async_trait]
impl Embedder for FakeEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, ApiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(bag_of_words(text))
    }
}

/// An embedder that always fails (to test error paths).
pub struct FailingEmbedder;

#[async_trait]
impl Embedder for FailingEmbedder {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, ApiError> {
        Err(ApiError::internal("embedding unavailable"))
    }
}

/// In-memory vector index with brute-force cosine distance.
#[derive(Default)]
pub struct FakeVectorStore {
    pub vectors: Mutex<BTreeMap<String, (Vec<f32>, Document)>>,
    /// Every filter passed to `query`, for assertions.
    pub filters: Mutex<Vec<Document>>,
}

impl FakeVectorStore {
    pub fn keys(&self) -> Vec<String> {
        self.vectors.lock().unwrap().keys().cloned().collect()
    }

    pub fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        self.keys()
            .into_iter()
            .filter(|k| k.starts_with(prefix))
            .collect()
    }

    pub fn metadata(&self, key: &str) -> Option<Document> {
        self.vectors
            .lock()
            .unwrap()
            .get(key)
            .map(|(_, m)| m.clone())
    }
}

fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 1.0;
    }
    1.0 - dot / (na * nb)
}

/// Evaluate the filter subset we use: `{field: {"$eq": value}}` and `{"$and": [...]}`.
/// Anything else is an error (so an unexpected filter shape fails loudly in tests).
pub fn filter_matches(filter: &Document, metadata: &Document) -> Result<bool, String> {
    let obj = filter.as_object().ok_or("filter must be an object")?;
    for (k, cond) in obj {
        let ok = if k == "$and" {
            let parts = cond.as_array().ok_or("$and needs an array")?;
            let mut all = true;
            for p in parts {
                all &= filter_matches(p, metadata)?;
            }
            all
        } else if k.starts_with('$') {
            return Err(format!("unsupported operator {k}"));
        } else {
            let c = cond.as_object().ok_or("condition must be an object")?;
            if c.len() != 1 {
                return Err("one operator per condition".into());
            }
            let want = c.get("$eq").ok_or("only $eq is supported")?;
            metadata.as_object().and_then(|m| m.get(k)) == Some(want)
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

#[async_trait]
impl VectorStore for FakeVectorStore {
    async fn put(&self, vectors: Vec<NewVector>) -> Result<(), ApiError> {
        let mut store = self.vectors.lock().unwrap();
        for v in vectors {
            store.insert(v.key, (v.data, v.metadata));
        }
        Ok(())
    }

    async fn delete(&self, keys: Vec<String>) -> Result<(), ApiError> {
        let mut store = self.vectors.lock().unwrap();
        for k in keys {
            store.remove(&k);
        }
        Ok(())
    }

    async fn query(
        &self,
        vector: Vec<f32>,
        top_k: i32,
        filter: Document,
    ) -> Result<Vec<VectorMatch>, ApiError> {
        self.filters.lock().unwrap().push(filter.clone());
        let store = self.vectors.lock().unwrap();
        let mut out = Vec::new();
        for (key, (data, meta)) in store.iter() {
            if filter_matches(&filter, meta).map_err(ApiError::internal)? {
                out.push(VectorMatch {
                    key: key.clone(),
                    distance: Some(cosine_distance(&vector, data)),
                    metadata: Some(meta.clone()),
                });
            }
        }
        out.sort_by(|a, b| a.distance.unwrap().total_cmp(&b.distance.unwrap()));
        out.truncate(usize::try_from(top_k).unwrap_or(0));
        Ok(out)
    }
}

/// AWS SDK clients pointing at `endpoint` with static test credentials.
pub fn aws_clients(endpoint: &str) -> (aws_sdk_dynamodb::Client, aws_sdk_s3::Client) {
    use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
    let ddb = aws_sdk_dynamodb::Client::from_conf(
        aws_sdk_dynamodb::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-west-2"))
            .credentials_provider(Credentials::new("test", "test", None, None, "test"))
            .endpoint_url(endpoint)
            .build(),
    );
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-west-2"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "test", "test", None, None, "test",
            ))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .build(),
    );
    (ddb, s3)
}

/// A minimal DynamoDB endpoint that answers `Query` (by `:pk`) from canned items in DynamoDB
/// JSON, and fails every other operation. Returns the endpoint URL.
pub async fn fake_dynamodb(items: Vec<serde_json::Value>) -> String {
    use axum::{
        Router,
        http::{HeaderMap, StatusCode, header},
        response::IntoResponse,
        routing::post,
    };
    use serde_json::{Value, json};
    use std::sync::Arc;

    let items = Arc::new(items);
    let app = Router::new().route(
        "/",
        post(move |headers: HeaderMap, body: String| {
            let items = Arc::clone(&items);
            async move {
                let target = headers
                    .get("x-amz-target")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default();
                let ct = [(header::CONTENT_TYPE, "application/x-amz-json-1.0")];
                if !target.ends_with(".Query") {
                    return (StatusCode::BAD_REQUEST, ct, "{}".to_owned()).into_response();
                }
                let req: Value = serde_json::from_str(&body).unwrap_or_default();
                let pk = req["ExpressionAttributeValues"][":pk"]["S"].clone();
                let mut matching: Vec<Value> = items
                    .iter()
                    .filter(|i| i["pk"]["S"] == pk)
                    .cloned()
                    .collect();
                matching.sort_by_key(|i| i["sk"]["S"].as_str().unwrap_or_default().to_owned());
                let out = json!({ "Items": matching, "Count": matching.len() });
                (StatusCode::OK, ct, out.to_string()).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// A note metadata item in DynamoDB JSON (for `fake_dynamodb`).
pub fn note_item_json(
    sub: &str,
    path: &str,
    title: &str,
    project: Option<&str>,
    updated_at: &str,
) -> serde_json::Value {
    let mut v = serde_json::json!({
        "pk": { "S": format!("NOTE#{sub}") }, "sk": { "S": path },
        "title": { "S": title },
        "contentHash": { "S": "a".repeat(64) }, "s3VersionId": { "S": "v1" },
        "size": { "N": "10" },
        "createdAt": { "S": updated_at }, "updatedAt": { "S": updated_at }
    });
    if let Some(p) = project {
        v["project"] = serde_json::json!({ "S": p });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::search_filter;
    use crate::vectors::ChunkMetadata;

    fn meta(sub: &str, project: Option<&str>) -> Document {
        ChunkMetadata {
            sub: sub.into(),
            path: "a.md".into(),
            project: project.map(str::to_owned),
            heading: "h".into(),
            snippet: "s".into(),
            title: "t".into(),
            updated_at: "u".into(),
        }
        .to_document()
    }

    #[test]
    fn filter_evaluation() {
        let f = search_filter("u1", None);
        assert_eq!(filter_matches(&f, &meta("u1", None)), Ok(true));
        assert_eq!(filter_matches(&f, &meta("u2", None)), Ok(false));
        let f = search_filter("u1", Some("p"));
        assert_eq!(filter_matches(&f, &meta("u1", Some("p"))), Ok(true));
        assert_eq!(filter_matches(&f, &meta("u1", Some("q"))), Ok(false));
        assert_eq!(filter_matches(&f, &meta("u1", None)), Ok(false));
        assert_eq!(filter_matches(&f, &meta("u2", Some("p"))), Ok(false));
        assert!(filter_matches(&Document::Null, &meta("u1", None)).is_err());
    }

    #[test]
    fn bag_of_words_similarity() {
        let a = bag_of_words("storage in S3 buckets");
        let b = bag_of_words("S3 storage");
        let c = bag_of_words("chocolate cake recipe");
        assert!(cosine_distance(&a, &b) < cosine_distance(&a, &c));
        assert_eq!(cosine_distance(&bag_of_words(""), &a), 1.0);
    }
}
