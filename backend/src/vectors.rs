//! The notes vector index (see "Vectors (S3 Vectors)" in API_CONTRACT.md).
//!
//! Key: `<sub>#<path>#<n>`. Filterable metadata: `sub`, `path`, `project` (omitted when null).
//! Non-filterable metadata: `heading`, `snippet`, `title`, `updatedAt`.

use std::{collections::HashMap, ops::Range};

use async_trait::async_trait;
use aws_sdk_s3vectors::{
    error::DisplayErrorContext,
    types::{PutInputVector, VectorData},
};
use aws_smithy_types::{Document, Number};

use crate::error::ApiError;

/// S3 Vectors accepts at most 500 vectors per `PutVectors` and 500 keys per `DeleteVectors`.
pub const MAX_BATCH: usize = 500;

/// Metadata keys that must be declared non-filterable on the index.
pub const NON_FILTERABLE_KEYS: [&str; 4] = ["heading", "snippet", "title", "updatedAt"];

/// Cap for the `heading` metadata value (characters), so deep heading paths stay small.
pub const MAX_HEADING_CHARS: usize = 500;

/// A vector to write.
#[derive(Debug, Clone, PartialEq)]
pub struct NewVector {
    pub key: String,
    pub data: Vec<f32>,
    pub metadata: Document,
}

/// One query result.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorMatch {
    pub key: String,
    pub distance: Option<f32>,
    pub metadata: Option<Document>,
}

/// The vector index operations used by the API and the indexer.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Upsert vectors (any number; implementations batch as needed).
    async fn put(&self, vectors: Vec<NewVector>) -> Result<(), ApiError>;
    /// Delete vectors by key (missing keys are not an error).
    async fn delete(&self, keys: Vec<String>) -> Result<(), ApiError>;
    /// Nearest neighbours of `vector` matching `filter`, with metadata and distance.
    async fn query(
        &self,
        vector: Vec<f32>,
        top_k: i32,
        filter: Document,
    ) -> Result<Vec<VectorMatch>, ApiError>;
}

/// Vector key: `<sub>#<path>#<n>`.
pub fn vector_key(sub: &str, path: &str, n: u32) -> String {
    format!("{sub}#{path}#{n}")
}

/// Keys `<sub>#<path>#<n>` for every `n` in `range` (empty if the range is empty).
pub fn chunk_keys(sub: &str, path: &str, range: Range<u32>) -> Vec<String> {
    range.map(|n| vector_key(sub, path, n)).collect()
}

/// The path encoded in a vector key, if the key belongs to `sub`.
pub fn path_from_key<'a>(sub: &str, key: &'a str) -> Option<&'a str> {
    let rest = key.strip_prefix(sub)?.strip_prefix('#')?;
    let (path, n) = rest.rsplit_once('#')?;
    n.parse::<u32>().ok().map(|_| path)
}

/// Metadata stored with each chunk vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkMetadata {
    pub sub: String,
    pub path: String,
    pub project: Option<String>,
    pub heading: String,
    pub snippet: String,
    pub title: String,
    pub updated_at: String,
}

impl ChunkMetadata {
    /// The metadata document. `project` is omitted when `None`; `heading` is capped.
    pub fn to_document(&self) -> Document {
        let mut m = HashMap::from([
            ("sub".to_owned(), Document::from(self.sub.as_str())),
            ("path".to_owned(), Document::from(self.path.as_str())),
            (
                "heading".to_owned(),
                Document::from(crate::embed::cap_chars(&self.heading, MAX_HEADING_CHARS)),
            ),
            ("snippet".to_owned(), Document::from(self.snippet.as_str())),
            ("title".to_owned(), Document::from(self.title.as_str())),
            (
                "updatedAt".to_owned(),
                Document::from(self.updated_at.as_str()),
            ),
        ]);
        if let Some(p) = &self.project {
            m.insert("project".to_owned(), Document::from(p.as_str()));
        }
        Document::Object(m)
    }
}

/// Metadata read back from a query result. Every field is optional: results are untrusted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HitMetadata {
    pub sub: Option<String>,
    pub path: Option<String>,
    pub project: Option<String>,
    pub heading: Option<String>,
    pub snippet: Option<String>,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

impl HitMetadata {
    pub fn from_document(doc: &Document) -> Self {
        let s = |k: &str| doc_str(doc, k).map(str::to_owned);
        HitMetadata {
            sub: s("sub"),
            path: s("path"),
            project: s("project"),
            heading: s("heading"),
            snippet: s("snippet"),
            title: s("title"),
            updated_at: s("updatedAt"),
        }
    }
}

/// A string field of an object document.
pub fn doc_str<'a>(doc: &'a Document, key: &str) -> Option<&'a str> {
    doc.as_object()?.get(key)?.as_string()
}

/// Convert a document to JSON (for tests and logging).
pub fn doc_to_json(doc: &Document) -> serde_json::Value {
    use serde_json::Value;
    match doc {
        Document::Object(m) => {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), doc_to_json(v))).collect())
        }
        Document::Array(a) => Value::Array(a.iter().map(doc_to_json).collect()),
        Document::Number(Number::PosInt(n)) => Value::from(*n),
        Document::Number(Number::NegInt(n)) => Value::from(*n),
        Document::Number(Number::Float(f)) => Value::from(*f),
        Document::String(s) => Value::String(s.clone()),
        Document::Bool(b) => Value::Bool(*b),
        Document::Null => Value::Null,
    }
}

/// Amazon S3 Vectors implementation.
pub struct S3VectorStore {
    client: aws_sdk_s3vectors::Client,
    bucket: String,
    index: String,
}

impl S3VectorStore {
    pub fn new(
        client: aws_sdk_s3vectors::Client,
        bucket: impl Into<String>,
        index: impl Into<String>,
    ) -> Self {
        Self {
            client,
            bucket: bucket.into(),
            index: index.into(),
        }
    }
}

fn s3v_err<E: std::error::Error>(op: &str, e: &E) -> ApiError {
    ApiError::internal(format!("{op} failed: {}", DisplayErrorContext(e)))
}

#[async_trait]
impl VectorStore for S3VectorStore {
    async fn put(&self, vectors: Vec<NewVector>) -> Result<(), ApiError> {
        for batch in vectors.chunks(MAX_BATCH) {
            let items = batch
                .iter()
                .map(|v| {
                    PutInputVector::builder()
                        .key(&v.key)
                        .data(VectorData::Float32(v.data.clone()))
                        .metadata(v.metadata.clone())
                        .build()
                        .map_err(ApiError::internal)
                })
                .collect::<Result<Vec<_>, _>>()?;
            self.client
                .put_vectors()
                .vector_bucket_name(&self.bucket)
                .index_name(&self.index)
                .set_vectors(Some(items))
                .send()
                .await
                .map_err(|e| s3v_err("PutVectors", &e))?;
        }
        Ok(())
    }

    async fn delete(&self, keys: Vec<String>) -> Result<(), ApiError> {
        for batch in keys.chunks(MAX_BATCH) {
            self.client
                .delete_vectors()
                .vector_bucket_name(&self.bucket)
                .index_name(&self.index)
                .set_keys(Some(batch.to_vec()))
                .send()
                .await
                .map_err(|e| s3v_err("DeleteVectors", &e))?;
        }
        Ok(())
    }

    async fn query(
        &self,
        vector: Vec<f32>,
        top_k: i32,
        filter: Document,
    ) -> Result<Vec<VectorMatch>, ApiError> {
        let out = self
            .client
            .query_vectors()
            .vector_bucket_name(&self.bucket)
            .index_name(&self.index)
            .top_k(top_k)
            .query_vector(VectorData::Float32(vector))
            .filter(filter)
            .return_metadata(true)
            .return_distance(true)
            .send()
            .await
            .map_err(|e| s3v_err("QueryVectors", &e))?;
        Ok(out
            .vectors()
            .iter()
            .map(|v| VectorMatch {
                key: v.key().to_owned(),
                distance: v.distance(),
                metadata: v.metadata().cloned(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(project: Option<&str>) -> ChunkMetadata {
        ChunkMetadata {
            sub: "u1".into(),
            path: "Projects/Hades/Design #1.md".into(),
            project: project.map(str::to_owned),
            heading: "Design > Storage".into(),
            snippet: "Content lives in S3".into(),
            title: "Design".into(),
            updated_at: "2026-10-19T15:00:00.000Z".into(),
        }
    }

    #[test]
    fn keys() {
        assert_eq!(vector_key("u1", "a/b.md", 3), "u1#a/b.md#3");
        assert_eq!(chunk_keys("u", "a.md", 1..3), ["u#a.md#1", "u#a.md#2"]);
        assert!(chunk_keys("u", "a.md", 3..3).is_empty());
        #[allow(clippy::reversed_empty_ranges)]
        let reversed = chunk_keys("u", "a.md", 5..2);
        assert!(reversed.is_empty());
    }

    #[test]
    fn path_from_keys() {
        assert_eq!(path_from_key("u1", "u1#a/#b.md#12"), Some("a/#b.md"));
        assert_eq!(path_from_key("u1", "u2#a.md#0"), None);
        assert_eq!(path_from_key("u1", "u1x#a.md#0"), None);
        assert_eq!(path_from_key("u1", "u1#a.md#x"), None);
        assert_eq!(path_from_key("u1", "u1#a.md"), None);
    }

    #[test]
    fn metadata_document_per_contract() {
        assert_eq!(
            doc_to_json(&meta(Some("hades")).to_document()),
            json!({
                "sub": "u1", "path": "Projects/Hades/Design #1.md", "project": "hades",
                "heading": "Design > Storage", "snippet": "Content lives in S3",
                "title": "Design", "updatedAt": "2026-10-19T15:00:00.000Z"
            })
        );
        let no_project = doc_to_json(&meta(None).to_document());
        assert!(no_project.get("project").is_none(), "{no_project}");
    }

    #[test]
    fn heading_is_capped() {
        let mut m = meta(None);
        m.heading = "h".repeat(MAX_HEADING_CHARS + 10);
        let doc = m.to_document();
        assert_eq!(
            doc_str(&doc, "heading").unwrap().chars().count(),
            MAX_HEADING_CHARS
        );
    }

    #[test]
    fn metadata_round_trip() {
        let back = HitMetadata::from_document(&meta(Some("p")).to_document());
        assert_eq!(back.sub.as_deref(), Some("u1"));
        assert_eq!(back.path.as_deref(), Some("Projects/Hades/Design #1.md"));
        assert_eq!(back.project.as_deref(), Some("p"));
        assert_eq!(back.heading.as_deref(), Some("Design > Storage"));
        assert_eq!(back.updated_at.as_deref(), Some("2026-10-19T15:00:00.000Z"));
        assert_eq!(
            HitMetadata::from_document(&Document::Null),
            HitMetadata::default()
        );
        // Wrong types are ignored rather than trusted.
        let odd = Document::Object(HashMap::from([("path".to_owned(), Document::from(3u64))]));
        assert_eq!(HitMetadata::from_document(&odd).path, None);
    }
}
