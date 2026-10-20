//! Notes storage: content in S3 (versioned), metadata in DynamoDB.
//!
//! Item layout: `pk` (S) = `NOTE#<sub>`, `sk` (S) = path, `title` (S), `project` (S, optional),
//! `tags` (L of S, optional), `contentHash` (S), `s3VersionId` (S), `size` (N),
//! `createdAt` (S), `updatedAt` (S). See "Notes (phase 1)" in API_CONTRACT.md.
//!
//! Phase 2 adds `indexedHash` (S) and `chunkCount` (N), written **only by the indexer**. The
//! API never writes them: create and move write fresh items without them, and update uses
//! `UpdateItem SET` on other attributes only, so they survive edits (and `indexed` turns false).
//!
//! Conflict responses carry `currentHash`, obtained from the failed conditional write itself
//! (`ReturnValuesOnConditionCheckFailure = ALL_OLD`), so no extra read is needed.

use std::collections::HashMap;

use aws_sdk_dynamodb::{
    Client as DdbClient,
    error::DisplayErrorContext,
    operation::{
        delete_item::DeleteItemError, put_item::PutItemError,
        transact_write_items::TransactWriteItemsError, update_item::UpdateItemError,
    },
    types::{
        AttributeValue, Delete, Put, ReturnValue, ReturnValuesOnConditionCheckFailure,
        TransactWriteItem,
    },
};
use aws_sdk_s3::{Client as S3Client, primitives::ByteStream};
use chrono::Utc;

use crate::{
    error::ApiError,
    models::{Note, NoteMeta},
    notes::{DerivedMeta, copy_source, derive_metadata, s3_key, sha256_hex},
    repo::journal::format_sk,
};

pub const PK_PREFIX: &str = "NOTE#";
pub const CONTENT_TYPE: &str = "text/markdown; charset=utf-8";
/// Indexer-owned attributes.
pub const ATTR_INDEXED_HASH: &str = "indexedHash";
pub const ATTR_CHUNK_COUNT: &str = "chunkCount";

const MSG_NOT_FOUND: &str = "note not found";
const MSG_EXISTS: &str = "note already exists";
const MSG_CHANGED: &str = "note was changed";
const MSG_TARGET_EXISTS: &str = "target exists";

/// Partition key for a user's notes.
pub fn pk_for(sub: &str) -> String {
    format!("{PK_PREFIX}{sub}")
}

/// A metadata item: the API-facing metadata plus the S3 version it points at and the
/// indexer-owned attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRecord {
    pub meta: NoteMeta,
    pub s3_version_id: String,
    /// `indexedHash`: the content hash last indexed, if any.
    pub indexed_hash: Option<String>,
    /// `chunkCount`: number of vectors stored for the note (0 if absent).
    pub chunk_count: u32,
}

impl NoteRecord {
    /// A record for a freshly written item (never indexed yet).
    pub fn fresh(meta: NoteMeta, s3_version_id: String) -> Self {
        Self {
            meta: NoteMeta {
                indexed: false,
                ..meta
            },
            s3_version_id,
            indexed_hash: None,
            chunk_count: 0,
        }
    }
}

/// Build the full DynamoDB item for a freshly written note. The indexer-owned attributes
/// (`indexedHash`, `chunkCount`) are deliberately never written here.
pub fn note_item(sub: &str, rec: &NoteRecord) -> HashMap<String, AttributeValue> {
    let m = &rec.meta;
    let mut item = HashMap::from([
        ("pk".to_owned(), AttributeValue::S(pk_for(sub))),
        ("sk".to_owned(), AttributeValue::S(m.path.clone())),
        ("title".to_owned(), AttributeValue::S(m.title.clone())),
        (
            "contentHash".to_owned(),
            AttributeValue::S(m.content_hash.clone()),
        ),
        (
            "s3VersionId".to_owned(),
            AttributeValue::S(rec.s3_version_id.clone()),
        ),
        ("size".to_owned(), AttributeValue::N(m.size.to_string())),
        (
            "createdAt".to_owned(),
            AttributeValue::S(m.created_at.clone()),
        ),
        (
            "updatedAt".to_owned(),
            AttributeValue::S(m.updated_at.clone()),
        ),
    ]);
    if let Some(p) = &m.project {
        item.insert("project".to_owned(), AttributeValue::S(p.clone()));
    }
    if !m.tags.is_empty() {
        item.insert("tags".to_owned(), tags_value(&m.tags));
    }
    item
}

fn tags_value(tags: &[String]) -> AttributeValue {
    AttributeValue::L(tags.iter().cloned().map(AttributeValue::S).collect())
}

/// Read a metadata item. Optional attributes are read tolerantly: `tags` may be missing, an
/// `L` (non-string elements skipped) or an `SS`; `indexedHash` / `chunkCount` may be missing.
pub fn record_from_item(item: &HashMap<String, AttributeValue>) -> Result<NoteRecord, ApiError> {
    let s = |name: &str| {
        item.get(name)
            .and_then(|v| v.as_s().ok())
            .cloned()
            .ok_or_else(|| ApiError::internal(format!("note item missing string attribute {name}")))
    };
    let size = item
        .get("size")
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or_else(|| ApiError::internal("note item missing numeric attribute size"))?;
    let project = item
        .get("project")
        .and_then(|v| v.as_s().ok())
        .filter(|p| !p.trim().is_empty())
        .cloned();
    let tags = match item.get("tags") {
        Some(AttributeValue::L(list)) => {
            list.iter().filter_map(|v| v.as_s().ok().cloned()).collect()
        }
        Some(AttributeValue::Ss(set)) => set.clone(),
        _ => Vec::new(),
    };
    let content_hash = s("contentHash")?;
    let indexed_hash = item
        .get(ATTR_INDEXED_HASH)
        .and_then(|v| v.as_s().ok())
        .cloned();
    let chunk_count = item
        .get(ATTR_CHUNK_COUNT)
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse::<u32>().ok())
        .unwrap_or(0);
    Ok(NoteRecord {
        meta: NoteMeta {
            path: s("sk")?,
            title: s("title")?,
            project,
            tags,
            indexed: indexed_hash.as_deref() == Some(content_hash.as_str()),
            content_hash,
            size,
            created_at: s("createdAt")?,
            updated_at: s("updatedAt")?,
        },
        s3_version_id: s("s3VersionId")?,
        indexed_hash,
        chunk_count,
    })
}

/// `contentHash` of an item returned by a failed condition check, if any.
fn current_hash(item: Option<&HashMap<String, AttributeValue>>) -> Option<String> {
    item.filter(|i| !i.is_empty())?
        .get("contentHash")
        .and_then(|v| v.as_s().ok())
        .cloned()
}

fn build_meta(path: &str, derived: DerivedMeta, content: &str, hash: String) -> NoteMeta {
    let now = format_sk(Utc::now());
    NoteMeta {
        path: path.to_owned(),
        title: derived.title,
        project: derived.project,
        tags: derived.tags,
        content_hash: hash,
        size: content.len() as u64,
        created_at: now.clone(),
        updated_at: now,
        indexed: false,
    }
}

/// `SET a, b REMOVE c` from the parts (the API's update never names indexer attributes).
fn update_expression(set: &[&str], remove: &[&str]) -> String {
    let mut expr = format!("SET {}", set.join(", "));
    if !remove.is_empty() {
        expr.push_str(&format!(" REMOVE {}", remove.join(", ")));
    }
    expr
}

/// Map an AWS SDK error (DynamoDB or S3) to a 500, keeping the full error chain in the log.
pub(crate) fn aws_err<E: std::error::Error>(op: &str, e: &E) -> ApiError {
    ApiError::internal(format!("{op} failed: {}", DisplayErrorContext(e)))
}

pub struct NotesRepo<'a> {
    ddb: &'a DdbClient,
    s3: &'a S3Client,
    table: &'a str,
    bucket: &'a str,
}

impl<'a> NotesRepo<'a> {
    pub fn new(ddb: &'a DdbClient, s3: &'a S3Client, table: &'a str, bucket: &'a str) -> Self {
        Self {
            ddb,
            s3,
            table,
            bucket,
        }
    }

    fn key(sub: &str, path: &str) -> HashMap<String, AttributeValue> {
        HashMap::from([
            ("pk".to_owned(), AttributeValue::S(pk_for(sub))),
            ("sk".to_owned(), AttributeValue::S(path.to_owned())),
        ])
    }

    // ---- S3 primitives -------------------------------------------------------------------

    /// `PutObject`; returns the new version id (the bucket must be versioned).
    async fn put_content(&self, key: &str, content: &str) -> Result<String, ApiError> {
        let out = self
            .s3
            .put_object()
            .bucket(self.bucket)
            .key(key)
            .content_type(CONTENT_TYPE)
            .body(ByteStream::from(content.as_bytes().to_vec()))
            .send()
            .await
            .map_err(|e| aws_err("PutObject", &e))?;
        match out.version_id() {
            Some(v) if v != "null" => Ok(v.to_owned()),
            _ => Err(ApiError::internal(
                "PutObject returned no VersionId: notes bucket must have versioning enabled",
            )),
        }
    }

    /// `GetObject` of one exact version, as UTF-8.
    pub async fn get_content(&self, key: &str, version_id: &str) -> Result<String, ApiError> {
        let out = self
            .s3
            .get_object()
            .bucket(self.bucket)
            .key(key)
            .version_id(version_id)
            .send()
            .await
            .map_err(|e| aws_err("GetObject", &e))?;
        let bytes = out
            .body
            .collect()
            .await
            .map_err(|e| ApiError::internal(format!("reading S3 body failed: {e}")))?
            .into_bytes();
        String::from_utf8(bytes.to_vec())
            .map_err(|_| ApiError::internal(format!("S3 object {key} is not valid UTF-8")))
    }

    /// Best-effort removal of a version we just wrote (permanent delete of that version, so
    /// the previous version, if any, becomes current again).
    async fn rollback_version(&self, key: &str, version_id: &str) {
        if let Err(e) = self
            .s3
            .delete_object()
            .bucket(self.bucket)
            .key(key)
            .version_id(version_id)
            .send()
            .await
        {
            tracing::error!(key, version_id, error = %DisplayErrorContext(&e), "S3 rollback failed");
        }
    }

    /// `DeleteObject` without a version id: adds a delete marker, history is kept.
    /// Best-effort, because the metadata (the source of truth) is already gone.
    async fn delete_current(&self, key: &str) {
        if let Err(e) = self
            .s3
            .delete_object()
            .bucket(self.bucket)
            .key(key)
            .send()
            .await
        {
            tracing::error!(key, error = %DisplayErrorContext(&e), "S3 delete (marker) failed");
        }
    }

    // ---- Reads ---------------------------------------------------------------------------

    /// All notes (optionally under a path prefix), sorted by path ascending.
    pub async fn list(&self, sub: &str, prefix: Option<&str>) -> Result<Vec<NoteMeta>, ApiError> {
        let mut notes = Vec::new();
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;
        let prefix = prefix.filter(|p| !p.is_empty());
        loop {
            let mut q = self
                .ddb
                .query()
                .table_name(self.table)
                .expression_attribute_values(":pk", AttributeValue::S(pk_for(sub)))
                .scan_index_forward(true)
                .set_exclusive_start_key(start_key.take());
            q = match prefix {
                Some(p) => q
                    .key_condition_expression("pk = :pk AND begins_with(sk, :prefix)")
                    .expression_attribute_values(":prefix", AttributeValue::S(p.to_owned())),
                None => q.key_condition_expression("pk = :pk"),
            };
            let out = q.send().await.map_err(|e| aws_err("Query", &e))?;
            for item in out.items() {
                notes.push(record_from_item(item)?.meta);
            }
            match out.last_evaluated_key() {
                Some(k) if !k.is_empty() => start_key = Some(k.clone()),
                _ => break,
            }
        }
        Ok(notes)
    }

    /// The metadata item for a path (strongly consistent).
    pub async fn get_record(&self, sub: &str, path: &str) -> Result<Option<NoteRecord>, ApiError> {
        let out = self
            .ddb
            .get_item()
            .table_name(self.table)
            .set_key(Some(Self::key(sub, path)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|e| aws_err("GetItem", &e))?;
        out.item()
            .filter(|i| !i.is_empty())
            .map(record_from_item)
            .transpose()
    }

    /// Metadata plus the content of exactly the S3 version the metadata points at.
    pub async fn get(&self, sub: &str, path: &str) -> Result<Option<Note>, ApiError> {
        let Some(rec) = self.get_record(sub, path).await? else {
            return Ok(None);
        };
        let content = self
            .get_content(&s3_key(sub, path), &rec.s3_version_id)
            .await?;
        Ok(Some(Note {
            meta: rec.meta,
            content,
        }))
    }

    // ---- Writes --------------------------------------------------------------------------

    /// Create-only. 409 (with `currentHash`) if the note already exists.
    pub async fn create(&self, sub: &str, path: &str, content: &str) -> Result<NoteMeta, ApiError> {
        let meta = build_meta(
            path,
            derive_metadata(path, content),
            content,
            sha256_hex(content),
        );
        let key = s3_key(sub, path);
        let version = self.put_content(&key, content).await?;
        let rec = NoteRecord::fresh(meta, version);
        let res = self
            .ddb
            .put_item()
            .table_name(self.table)
            .set_item(Some(note_item(sub, &rec)))
            .condition_expression("attribute_not_exists(sk)")
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .send()
            .await;
        match res {
            Ok(_) => Ok(rec.meta),
            Err(e) => {
                self.rollback_version(&key, &rec.s3_version_id).await;
                match e.as_service_error() {
                    Some(PutItemError::ConditionalCheckFailedException(c)) => {
                        Err(ApiError::conflict(MSG_EXISTS, current_hash(c.item())))
                    }
                    _ => Err(aws_err("PutItem (create)", &e)),
                }
            }
        }
    }

    /// Update-only, conditional on the stored hash equalling `base_hash` (lowercase).
    /// 404 if missing, 409 (with `currentHash`) on mismatch. Identical content is a no-op.
    pub async fn update(
        &self,
        sub: &str,
        path: &str,
        content: &str,
        base_hash: &str,
    ) -> Result<NoteMeta, ApiError> {
        let hash = sha256_hex(content);
        if hash == base_hash {
            // Same content as the caller's base: if that is still what's stored, nothing to do.
            let rec = self
                .get_record(sub, path)
                .await?
                .ok_or_else(|| ApiError::not_found(MSG_NOT_FOUND))?;
            if rec.meta.content_hash != base_hash {
                return Err(ApiError::conflict(MSG_CHANGED, Some(rec.meta.content_hash)));
            }
            return Ok(rec.meta);
        }

        let derived = derive_metadata(path, content);
        let key = s3_key(sub, path);
        let version = self.put_content(&key, content).await?;
        let now = format_sk(Utc::now());

        // UpdateItem (not PutItem) so `createdAt` is kept without reading the item first;
        // ALL_NEW returns the stored `createdAt`.
        let mut set = vec![
            "#title = :title",
            "#hash = :hash",
            "#ver = :ver",
            "#size = :size",
            "#updated = :updated",
        ];
        let mut remove = Vec::new();
        let mut req = self
            .ddb
            .update_item()
            .table_name(self.table)
            .set_key(Some(Self::key(sub, path)))
            .condition_expression("#hash = :base")
            .expression_attribute_names("#title", "title")
            .expression_attribute_names("#hash", "contentHash")
            .expression_attribute_names("#ver", "s3VersionId")
            .expression_attribute_names("#size", "size")
            .expression_attribute_names("#updated", "updatedAt")
            .expression_attribute_names("#project", "project")
            .expression_attribute_names("#tags", "tags")
            .expression_attribute_values(":title", AttributeValue::S(derived.title))
            .expression_attribute_values(":hash", AttributeValue::S(hash))
            .expression_attribute_values(":ver", AttributeValue::S(version.clone()))
            .expression_attribute_values(":size", AttributeValue::N(content.len().to_string()))
            .expression_attribute_values(":updated", AttributeValue::S(now))
            .expression_attribute_values(":base", AttributeValue::S(base_hash.to_owned()));
        match derived.project {
            Some(p) => {
                set.push("#project = :project");
                req = req.expression_attribute_values(":project", AttributeValue::S(p));
            }
            None => remove.push("#project"),
        }
        if derived.tags.is_empty() {
            remove.push("#tags");
        } else {
            set.push("#tags = :tags");
            req = req.expression_attribute_values(":tags", tags_value(&derived.tags));
        }
        let expr = update_expression(&set, &remove);

        let res = req
            .update_expression(expr)
            .return_values(ReturnValue::AllNew)
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .send()
            .await;
        match res {
            Ok(out) => {
                let attrs = out
                    .attributes()
                    .ok_or_else(|| ApiError::internal("UpdateItem returned no attributes"))?;
                Ok(record_from_item(attrs)?.meta)
            }
            Err(e) => {
                self.rollback_version(&key, &version).await;
                match e.as_service_error() {
                    Some(UpdateItemError::ConditionalCheckFailedException(c)) => {
                        match current_hash(c.item()) {
                            None => Err(ApiError::not_found(MSG_NOT_FOUND)),
                            Some(h) => Err(ApiError::conflict(MSG_CHANGED, Some(h))),
                        }
                    }
                    _ => Err(aws_err("UpdateItem", &e)),
                }
            }
        }
    }

    /// Conditional metadata delete, then an S3 delete marker.
    pub async fn delete(&self, sub: &str, path: &str, base_hash: &str) -> Result<(), ApiError> {
        let res = self
            .ddb
            .delete_item()
            .table_name(self.table)
            .set_key(Some(Self::key(sub, path)))
            .condition_expression("contentHash = :base")
            .expression_attribute_values(":base", AttributeValue::S(base_hash.to_owned()))
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .send()
            .await;
        if let Err(e) = res {
            return match e.as_service_error() {
                Some(DeleteItemError::ConditionalCheckFailedException(c)) => {
                    match current_hash(c.item()) {
                        None => Err(ApiError::not_found(MSG_NOT_FOUND)),
                        Some(h) => Err(ApiError::conflict(MSG_CHANGED, Some(h))),
                    }
                }
                _ => Err(aws_err("DeleteItem", &e)),
            };
        }
        self.delete_current(&s3_key(sub, path)).await;
        Ok(())
    }

    /// Move/rename `from` to `to` (paths already validated and different).
    pub async fn move_note(
        &self,
        sub: &str,
        from: &str,
        to: &str,
        base_hash: &str,
    ) -> Result<NoteMeta, ApiError> {
        // Read the source: fail fast (no S3 write) on 404 / 409, and we need its content to
        // re-derive the title (it may fall back to the filename) and its `createdAt`.
        let src = self
            .get_record(sub, from)
            .await?
            .ok_or_else(|| ApiError::not_found(MSG_NOT_FOUND))?;
        if src.meta.content_hash != base_hash {
            return Err(ApiError::conflict(MSG_CHANGED, Some(src.meta.content_hash)));
        }
        let from_key = s3_key(sub, from);
        let to_key = s3_key(sub, to);
        let content = self.get_content(&from_key, &src.s3_version_id).await?;
        let derived = derive_metadata(to, &content);

        // 1. Copy the exact source version to the new key.
        let copied = self
            .s3
            .copy_object()
            .bucket(self.bucket)
            .key(&to_key)
            .copy_source(copy_source(self.bucket, &from_key, &src.s3_version_id))
            .send()
            .await
            .map_err(|e| aws_err("CopyObject", &e))?;
        let new_version = match copied.version_id() {
            Some(v) if v != "null" => v.to_owned(),
            _ => {
                return Err(ApiError::internal(
                    "CopyObject returned no VersionId: notes bucket must have versioning enabled",
                ));
            }
        };

        let rec = NoteRecord::fresh(
            NoteMeta {
                path: to.to_owned(),
                title: derived.title,
                project: derived.project,
                tags: derived.tags,
                content_hash: src.meta.content_hash.clone(),
                size: src.meta.size,
                created_at: src.meta.created_at.clone(),
                updated_at: format_sk(Utc::now()),
                indexed: false,
            },
            new_version.clone(),
        );

        // 2. Create `to` + delete `from` atomically.
        let put = Put::builder()
            .table_name(self.table)
            .set_item(Some(note_item(sub, &rec)))
            .condition_expression("attribute_not_exists(sk)")
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .build()
            .map_err(ApiError::internal)?;
        let delete = Delete::builder()
            .table_name(self.table)
            .set_key(Some(Self::key(sub, from)))
            .condition_expression("contentHash = :base")
            .expression_attribute_values(":base", AttributeValue::S(base_hash.to_owned()))
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .build()
            .map_err(ApiError::internal)?;
        let res = self
            .ddb
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().put(put).build())
            .transact_items(TransactWriteItem::builder().delete(delete).build())
            .send()
            .await;

        if let Err(e) = res {
            // 4. Undo the copy.
            self.rollback_version(&to_key, &new_version).await;
            if let Some(TransactWriteItemsError::TransactionCanceledException(t)) =
                e.as_service_error()
            {
                let reasons = t.cancellation_reasons();
                let failed = |i: usize| {
                    reasons
                        .get(i)
                        .filter(|r| r.code() == Some("ConditionalCheckFailed"))
                };
                if let Some(r) = failed(1) {
                    return Err(match current_hash(r.item()) {
                        None => ApiError::not_found(MSG_NOT_FOUND),
                        Some(h) => ApiError::conflict(MSG_CHANGED, Some(h)),
                    });
                }
                if failed(0).is_some() {
                    return Err(ApiError::conflict(MSG_TARGET_EXISTS, None));
                }
            }
            return Err(aws_err("TransactWriteItems (move)", &e));
        }

        // 3. Delete marker on the old key.
        self.delete_current(&from_key).await;
        Ok(rec.meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(project: Option<&str>, tags: &[&str]) -> NoteRecord {
        NoteRecord {
            meta: NoteMeta {
                path: "Projects/Hades/Design.md".into(),
                title: "Design".into(),
                project: project.map(str::to_owned),
                tags: tags.iter().map(|t| t.to_string()).collect(),
                content_hash: "ab".repeat(32),
                size: 1234,
                created_at: "2026-10-19T14:03:22.123Z".into(),
                updated_at: "2026-10-19T15:00:00.000Z".into(),
                indexed: false,
            },
            s3_version_id: "v1".into(),
            indexed_hash: None,
            chunk_count: 0,
        }
    }

    #[test]
    fn pk_builder() {
        assert_eq!(pk_for("abc"), "NOTE#abc");
    }

    #[test]
    fn item_round_trip_full() {
        let rec = sample(Some("hades"), &["arch", "x"]);
        let item = note_item("u", &rec);
        assert_eq!(item["pk"].as_s().unwrap(), "NOTE#u");
        assert_eq!(item["sk"].as_s().unwrap(), "Projects/Hades/Design.md");
        assert_eq!(item["size"].as_n().unwrap(), "1234");
        assert_eq!(item["s3VersionId"].as_s().unwrap(), "v1");
        assert_eq!(item["tags"].as_l().unwrap().len(), 2);
        assert_eq!(record_from_item(&item).unwrap(), rec);
    }

    #[test]
    fn item_round_trip_minimal() {
        let rec = sample(None, &[]);
        let item = note_item("u", &rec);
        assert!(!item.contains_key("project"));
        assert!(!item.contains_key("tags"));
        let back = record_from_item(&item).unwrap();
        assert_eq!(back, rec);
        assert_eq!(back.meta.project, None);
        assert!(back.meta.tags.is_empty());
    }

    #[test]
    fn tags_read_tolerantly() {
        let mut item = note_item("u", &sample(None, &[]));
        item.insert(
            "tags".into(),
            AttributeValue::L(vec![
                AttributeValue::S("a".into()),
                AttributeValue::N("1".into()),
                AttributeValue::S("b".into()),
            ]),
        );
        assert_eq!(record_from_item(&item).unwrap().meta.tags, ["a", "b"]);
        item.insert("tags".into(), AttributeValue::Ss(vec!["z".into()]));
        assert_eq!(record_from_item(&item).unwrap().meta.tags, ["z"]);
        item.insert("tags".into(), AttributeValue::S("weird".into()));
        assert!(record_from_item(&item).unwrap().meta.tags.is_empty());
    }

    #[test]
    fn missing_required_attribute_is_internal() {
        let mut item = note_item("u", &sample(None, &[]));
        item.remove("contentHash");
        assert!(matches!(
            record_from_item(&item),
            Err(ApiError::Internal(_))
        ));
        let mut item = note_item("u", &sample(None, &[]));
        item.insert("size".into(), AttributeValue::S("12".into()));
        assert!(record_from_item(&item).is_err());
    }

    #[test]
    fn fresh_items_have_no_indexer_attributes() {
        let mut rec = sample(Some("p"), &["t"]);
        rec.indexed_hash = Some("ab".repeat(32));
        rec.chunk_count = 4;
        let item = note_item("u", &rec);
        assert!(!item.contains_key(ATTR_INDEXED_HASH));
        assert!(!item.contains_key(ATTR_CHUNK_COUNT));
        let fresh = NoteRecord::fresh(rec.meta.clone(), "v2".into());
        assert_eq!(fresh.indexed_hash, None);
        assert_eq!(fresh.chunk_count, 0);
        assert!(!fresh.meta.indexed);
    }

    #[test]
    fn indexed_flag_from_item() {
        let mut item = note_item("u", &sample(None, &[]));
        let rec = record_from_item(&item).unwrap();
        assert!(!rec.meta.indexed);
        assert_eq!((rec.indexed_hash, rec.chunk_count), (None, 0));

        item.insert(ATTR_INDEXED_HASH.into(), AttributeValue::S("ab".repeat(32)));
        item.insert(ATTR_CHUNK_COUNT.into(), AttributeValue::N("3".into()));
        let rec = record_from_item(&item).unwrap();
        assert!(rec.meta.indexed);
        assert_eq!(rec.chunk_count, 3);

        item.insert(ATTR_INDEXED_HASH.into(), AttributeValue::S("cd".repeat(32)));
        let rec = record_from_item(&item).unwrap();
        assert!(!rec.meta.indexed, "stale index");
        assert_eq!(rec.indexed_hash, Some("cd".repeat(32)));
    }

    #[test]
    fn update_expression_shape() {
        let e = update_expression(&["#a = :a", "#b = :b"], &["#c"]);
        assert_eq!(e, "SET #a = :a, #b = :b REMOVE #c");
        assert_eq!(update_expression(&["#a = :a"], &[]), "SET #a = :a");
    }

    #[test]
    fn current_hash_from_old_item() {
        let item = note_item("u", &sample(None, &[]));
        assert_eq!(current_hash(Some(&item)), Some("ab".repeat(32)));
        assert_eq!(current_hash(None), None);
        assert_eq!(current_hash(Some(&HashMap::new())), None);
    }

    #[test]
    fn meta_serializes_per_contract() {
        let v = serde_json::to_value(sample(None, &[]).meta).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "path": "Projects/Hades/Design.md", "title": "Design", "project": null,
                "tags": [], "contentHash": "ab".repeat(32), "size": 1234,
                "createdAt": "2026-10-19T14:03:22.123Z", "updatedAt": "2026-10-19T15:00:00.000Z",
                "indexed": false
            })
        );
        let note = Note {
            meta: sample(Some("p"), &["t"]).meta,
            content: "hi".into(),
        };
        let v = serde_json::to_value(note).unwrap();
        assert_eq!(v["content"], "hi");
        assert_eq!(v["project"], "p");
        assert_eq!(v["tags"], serde_json::json!(["t"]));
    }

    #[test]
    fn build_meta_sets_hash_size_and_timestamps() {
        let m = build_meta("a.md", derive_metadata("a.md", "€"), "€", sha256_hex("€"));
        assert_eq!(m.size, 3);
        assert_eq!(m.title, "a");
        assert_eq!(m.created_at, m.updated_at);
        assert_eq!(m.content_hash.len(), 64);
    }
}
