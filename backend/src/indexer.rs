//! The search indexer (see "Indexing pipeline" in API_CONTRACT.md). It reconciles the vector
//! index with the notes metadata, driven by DynamoDB stream records (or a full backfill).
//!
//! - **REMOVE:** delete vectors `#0 .. #chunkCount-1` (from the old image).
//! - **INSERT / MODIFY:** nothing to do if `indexedHash == contentHash`. Otherwise fetch the
//!   exact S3 version, chunk, embed, `PutVectors`, delete stale keys `#new .. #old-1`, and write
//!   back `indexedHash` / `chunkCount` conditional on `contentHash` being unchanged.
//!
//! Everything is idempotent, so retries and duplicate records are safe.

use std::{collections::HashMap, sync::Arc};

use aws_lambda_events::{
    dynamodb::{Event as StreamEvent, EventRecord},
    streams::{DynamoDbBatchItemFailure, DynamoDbEventResponse},
};
use aws_sdk_dynamodb::{operation::update_item::UpdateItemError, types::AttributeValue};
use serde::Serialize;
use serde_json::Value;
use tokio::task::JoinSet;

use crate::{
    chunker::chunk_note,
    embed::Embedder,
    error::ApiError,
    repo::notes::{
        ATTR_CHUNK_COUNT, ATTR_INDEXED_HASH, NoteRecord, NotesRepo, PK_PREFIX, aws_err,
        record_from_item,
    },
    vectors::{ChunkMetadata, NewVector, VectorStore, chunk_keys, vector_key},
};

/// Number of chunks embedded concurrently.
const EMBED_CONCURRENCY: usize = 8;

pub type Item = HashMap<String, AttributeValue>;

/// What a stream record asks the indexer to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The note is gone: delete its `chunk_count` vectors.
    Remove {
        sub: String,
        path: String,
        chunk_count: u32,
    },
    /// The note exists and its index is stale. `stale_count` is the number of vectors that may
    /// currently be stored for it.
    Reindex {
        sub: String,
        record: Box<NoteRecord>,
        stale_count: u32,
    },
    /// Nothing to do (already indexed, or not a note).
    Skip(&'static str),
}

/// What reconciling did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Removed {
        vectors: u32,
    },
    Indexed {
        chunks: u32,
    },
    /// The note changed (or was deleted) while indexing; a newer record will handle it.
    Superseded {
        chunks: u32,
    },
    Skipped(&'static str),
}

fn str_attr<'a>(item: Option<&'a Item>, name: &str) -> Option<&'a str> {
    item?.get(name)?.as_s().ok().map(String::as_str)
}

fn chunk_count(item: Option<&Item>) -> u32 {
    item.and_then(|i| i.get(ATTR_CHUNK_COUNT))
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Interpret a stream record (`eventName`, keys, and the old/new images; empty images count as
/// absent).
pub fn change_from_images(
    event_name: &str,
    keys: Option<&Item>,
    old: Option<&Item>,
    new: Option<&Item>,
) -> Result<Change, ApiError> {
    fn non_empty(i: Option<&Item>) -> Option<&Item> {
        i.filter(|i| !i.is_empty())
    }
    let (keys, old, new) = (non_empty(keys), non_empty(old), non_empty(new));
    let key_attr = |name: &str| {
        str_attr(keys, name)
            .or_else(|| str_attr(new, name))
            .or_else(|| str_attr(old, name))
    };
    let (Some(pk), Some(path)) = (key_attr("pk"), key_attr("sk")) else {
        return Err(ApiError::internal("stream record has no pk/sk"));
    };
    let Some(sub) = pk.strip_prefix(PK_PREFIX) else {
        return Ok(Change::Skip("not a note"));
    };
    match event_name {
        "REMOVE" => Ok(Change::Remove {
            sub: sub.to_owned(),
            path: path.to_owned(),
            chunk_count: chunk_count(old),
        }),
        "INSERT" | "MODIFY" => {
            let new = new.ok_or_else(|| {
                ApiError::internal(format!("{event_name} record without NewImage"))
            })?;
            let record = record_from_item(new)?;
            if record.meta.indexed {
                return Ok(Change::Skip("already indexed"));
            }
            let stale_count = record.chunk_count.max(chunk_count(old));
            Ok(Change::Reindex {
                sub: sub.to_owned(),
                record: Box::new(record),
                stale_count,
            })
        }
        other => Err(ApiError::internal(format!("unknown eventName {other:?}"))),
    }
}

/// Interpret a Lambda DynamoDB stream record.
pub fn change_from_record(rec: &EventRecord) -> Result<Change, ApiError> {
    let conv = |i: &serde_dynamo::Item| -> Item { i.clone().into() };
    let keys = conv(&rec.change.keys);
    let old = conv(&rec.change.old_image);
    let new = conv(&rec.change.new_image);
    change_from_images(&rec.event_name, Some(&keys), Some(&old), Some(&new))
}

/// A direct invocation payload.
#[derive(Debug)]
pub enum Invocation {
    Stream(StreamEvent),
    Backfill,
}

/// Dispatch on the raw payload: `{"backfill": true}` or a DynamoDB stream event.
pub fn parse_invocation(payload: Value) -> Result<Invocation, ApiError> {
    if payload.get("backfill") == Some(&Value::Bool(true)) {
        return Ok(Invocation::Backfill);
    }
    if payload.get("Records").is_some() {
        return serde_json::from_value(payload)
            .map(Invocation::Stream)
            .map_err(|e| ApiError::bad_request(format!("invalid DynamoDB stream event: {e}")));
    }
    Err(ApiError::bad_request(
        "expected a DynamoDB stream event or {\"backfill\": true}",
    ))
}

/// Partial batch response: report the failed record and every record after it (by
/// `SequenceNumber`), since processing stops at the first failure to keep per-item order.
pub fn batch_failures<'a>(
    failed: impl IntoIterator<Item = &'a EventRecord>,
) -> DynamoDbEventResponse {
    let mut resp = DynamoDbEventResponse::default();
    for rec in failed {
        let mut f = DynamoDbBatchItemFailure::default();
        f.item_identifier = rec.change.sequence_number.clone();
        resp.batch_item_failures.push(f);
    }
    resp
}

/// Counts reported by a backfill.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillReport {
    pub scanned: u32,
    pub up_to_date: u32,
    pub reindexed: u32,
    pub superseded: u32,
    pub failed: u32,
}

pub struct Indexer {
    pub ddb: aws_sdk_dynamodb::Client,
    pub s3: aws_sdk_s3::Client,
    pub table: String,
    pub bucket: String,
    pub embedder: Arc<dyn Embedder>,
    pub vectors: Arc<dyn VectorStore>,
}

impl Indexer {
    fn key(sub: &str, path: &str) -> Item {
        HashMap::from([
            (
                "pk".to_owned(),
                AttributeValue::S(format!("{PK_PREFIX}{sub}")),
            ),
            ("sk".to_owned(), AttributeValue::S(path.to_owned())),
        ])
    }

    /// Handle one invocation payload; returns the Lambda response.
    pub async fn handle(&self, payload: Value) -> Result<Value, ApiError> {
        match parse_invocation(payload)? {
            Invocation::Stream(event) => {
                let resp = self.handle_stream(&event).await;
                serde_json::to_value(resp).map_err(ApiError::internal)
            }
            Invocation::Backfill => {
                let report = self.backfill().await?;
                serde_json::to_value(report).map_err(ApiError::internal)
            }
        }
    }

    /// Process stream records in order; stop at the first failure and report it (and the rest)
    /// as batch item failures so the stream retries them.
    pub async fn handle_stream(&self, event: &StreamEvent) -> DynamoDbEventResponse {
        for (i, rec) in event.records.iter().enumerate() {
            let seq = rec.change.sequence_number.as_deref().unwrap_or("?");
            let res = match change_from_record(rec) {
                Ok(change) => self.apply(&change).await,
                Err(e) => Err(e),
            };
            match res {
                Ok(outcome) => {
                    tracing::info!(seq, event = %rec.event_name, ?outcome, "reconciled")
                }
                Err(e) => {
                    tracing::error!(seq, event = %rec.event_name, error = %e, "reconcile failed");
                    return batch_failures(&event.records[i..]);
                }
            }
        }
        DynamoDbEventResponse::default()
    }

    /// Apply one change.
    pub async fn apply(&self, change: &Change) -> Result<Outcome, ApiError> {
        match change {
            Change::Skip(why) => Ok(Outcome::Skipped(why)),
            Change::Remove {
                sub,
                path,
                chunk_count,
            } => {
                let keys = chunk_keys(sub, path, 0..*chunk_count);
                if !keys.is_empty() {
                    self.vectors.delete(keys).await?;
                }
                Ok(Outcome::Removed {
                    vectors: *chunk_count,
                })
            }
            Change::Reindex {
                sub,
                record,
                stale_count,
            } => self.reindex(sub, record, *stale_count).await,
        }
    }

    /// Embed every chunk text, `EMBED_CONCURRENCY` at a time, keeping order.
    async fn embed_all(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, ApiError> {
        let mut out = Vec::with_capacity(texts.len());
        let mut texts = texts.into_iter().peekable();
        while texts.peek().is_some() {
            let mut set = JoinSet::new();
            for (i, text) in texts.by_ref().take(EMBED_CONCURRENCY).enumerate() {
                let embedder = Arc::clone(&self.embedder);
                set.spawn(async move { (i, embedder.embed(&text).await) });
            }
            let mut batch: Vec<Option<Vec<f32>>> = vec![None; set.len()];
            while let Some(joined) = set.join_next().await {
                let (i, res) = joined.map_err(ApiError::internal)?;
                batch[i] = Some(res?);
            }
            out.extend(batch.into_iter().flatten());
        }
        Ok(out)
    }

    /// Reindex one note from the exact S3 version its metadata points at.
    pub async fn reindex(
        &self,
        sub: &str,
        record: &NoteRecord,
        stale_count: u32,
    ) -> Result<Outcome, ApiError> {
        let meta = &record.meta;
        let repo = NotesRepo::new(&self.ddb, &self.s3, &self.table, &self.bucket);
        let content = repo
            .get_content(
                &crate::notes::s3_key(sub, &meta.path),
                &record.s3_version_id,
            )
            .await?;

        let chunks = chunk_note(&meta.title, &content);
        let count = u32::try_from(chunks.len()).map_err(ApiError::internal)?;
        let embeddings = self
            .embed_all(chunks.iter().map(|c| c.embed_text(&meta.title)).collect())
            .await?;
        let vectors: Vec<NewVector> = chunks
            .iter()
            .zip(embeddings)
            .zip(0u32..)
            .map(|((chunk, data), n)| NewVector {
                key: vector_key(sub, &meta.path, n),
                data,
                metadata: ChunkMetadata {
                    sub: sub.to_owned(),
                    path: meta.path.clone(),
                    project: meta.project.clone(),
                    heading: chunk.heading.clone(),
                    snippet: chunk.snippet(),
                    title: meta.title.clone(),
                    updated_at: meta.updated_at.clone(),
                }
                .to_document(),
            })
            .collect();
        if !vectors.is_empty() {
            self.vectors.put(vectors).await?;
        }
        let stale = chunk_keys(sub, &meta.path, count..stale_count);
        if !stale.is_empty() {
            self.vectors.delete(stale).await?;
        }

        if self
            .write_back(sub, &meta.path, &meta.content_hash, count)
            .await?
        {
            Ok(Outcome::Indexed { chunks: count })
        } else {
            self.cleanup_superseded(sub, &meta.path, count).await?;
            Ok(Outcome::Superseded { chunks: count })
        }
    }

    /// `SET indexedHash, chunkCount` if `contentHash` is still `hash`. Returns false if the
    /// condition failed (the note changed or was deleted meanwhile).
    async fn write_back(
        &self,
        sub: &str,
        path: &str,
        hash: &str,
        count: u32,
    ) -> Result<bool, ApiError> {
        let res = self
            .ddb
            .update_item()
            .table_name(&self.table)
            .set_key(Some(Self::key(sub, path)))
            .update_expression("SET #ih = :h, #cc = :n")
            .condition_expression("#hash = :h")
            .expression_attribute_names("#ih", ATTR_INDEXED_HASH)
            .expression_attribute_names("#cc", ATTR_CHUNK_COUNT)
            .expression_attribute_names("#hash", "contentHash")
            .expression_attribute_values(":h", AttributeValue::S(hash.to_owned()))
            .expression_attribute_values(":n", AttributeValue::N(count.to_string()))
            .send()
            .await;
        match res {
            Ok(_) => Ok(true),
            Err(e) => match e.as_service_error() {
                Some(UpdateItemError::ConditionalCheckFailedException(_)) => Ok(false),
                _ => Err(aws_err("UpdateItem (index write-back)", &e)),
            },
        }
    }

    /// After a failed write-back, delete the vectors we wrote beyond what the item records
    /// (`#recorded .. #count-1`): no later record would know about them. Vectors below
    /// `recorded` are handled by the newer record (or the REMOVE record if the note is gone).
    async fn cleanup_superseded(&self, sub: &str, path: &str, count: u32) -> Result<(), ApiError> {
        let out = self
            .ddb
            .get_item()
            .table_name(&self.table)
            .set_key(Some(Self::key(sub, path)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|e| aws_err("GetItem (index cleanup)", &e))?;
        let recorded = chunk_count(out.item());
        let orphans = chunk_keys(sub, path, recorded..count);
        if !orphans.is_empty() {
            self.vectors.delete(orphans).await?;
        }
        Ok(())
    }

    /// Re-read a note and reindex its current version unconditionally (even if it is marked
    /// indexed), clearing vectors up to `written` that a superseded run may have left behind.
    /// A note that no longer exists has its leftover vectors deleted.
    pub async fn reindex_current(
        &self,
        sub: &str,
        path: &str,
        written: u32,
    ) -> Result<Outcome, ApiError> {
        let repo = NotesRepo::new(&self.ddb, &self.s3, &self.table, &self.bucket);
        match repo.get_record(sub, path).await? {
            Some(current) => {
                let stale = current.chunk_count.max(written);
                self.reindex(sub, &current, stale).await
            }
            None => {
                let keys = chunk_keys(sub, path, 0..written);
                if !keys.is_empty() {
                    self.vectors.delete(keys).await?;
                }
                Ok(Outcome::Removed { vectors: written })
            }
        }
    }

    /// Reindex every note whose `indexedHash != contentHash`. Per-note failures are counted
    /// and logged, not fatal.
    pub async fn backfill(&self) -> Result<BackfillReport, ApiError> {
        let mut report = BackfillReport::default();
        let mut start_key: Option<Item> = None;
        loop {
            let out = self
                .ddb
                .scan()
                .table_name(&self.table)
                .filter_expression("begins_with(pk, :p)")
                .expression_attribute_values(":p", AttributeValue::S(PK_PREFIX.to_owned()))
                .set_exclusive_start_key(start_key.take())
                .send()
                .await
                .map_err(|e| aws_err("Scan (backfill)", &e))?;
            for item in out.items() {
                report.scanned += 1;
                let res = match change_from_images("MODIFY", None, None, Some(item)) {
                    Ok(Change::Skip(_)) => {
                        report.up_to_date += 1;
                        continue;
                    }
                    Ok(change) => self.apply(&change).await,
                    Err(e) => Err(e),
                };
                // The scan snapshot can be older than the stream: if the note changed after the
                // scan, the stream may already have indexed the newer version, and the stale
                // vectors we just wrote would overwrite it. Reindex the current version once.
                let res = match res {
                    Ok(Outcome::Superseded { chunks }) => {
                        report.superseded += 1;
                        let path = str_attr(Some(item), "sk").unwrap_or_default().to_owned();
                        let sub = str_attr(Some(item), "pk")
                            .and_then(|pk| pk.strip_prefix(PK_PREFIX))
                            .unwrap_or_default()
                            .to_owned();
                        self.reindex_current(&sub, &path, chunks).await
                    }
                    other => other,
                };
                match res {
                    Ok(Outcome::Superseded { .. }) => {}
                    Ok(_) => report.reindexed += 1,
                    Err(e) => {
                        report.failed += 1;
                        let path = str_attr(Some(item), "sk").unwrap_or("?");
                        tracing::error!(path, error = %e, "backfill reindex failed");
                    }
                }
            }
            match out.last_evaluated_key() {
                Some(k) if !k.is_empty() => start_key = Some(k.clone()),
                _ => break,
            }
        }
        tracing::info!(?report, "backfill done");
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn image(path: &str, hash: &str, indexed: Option<&str>, chunks: Option<u32>) -> Value {
        let mut v = json!({
            "pk": { "S": "NOTE#user-1" }, "sk": { "S": path },
            "title": { "S": "Design" }, "project": { "S": "hades" },
            "tags": { "L": [ { "S": "arch" } ] },
            "contentHash": { "S": hash }, "s3VersionId": { "S": "v1" },
            "size": { "N": "12" },
            "createdAt": { "S": "2026-10-19T14:03:22.123Z" },
            "updatedAt": { "S": "2026-10-19T15:00:00.000Z" }
        });
        if let Some(h) = indexed {
            v["indexedHash"] = json!({ "S": h });
        }
        if let Some(n) = chunks {
            v["chunkCount"] = json!({ "N": n.to_string() });
        }
        v
    }

    fn record(name: &str, seq: &str, old: Option<Value>, new: Option<Value>) -> Value {
        let path = new
            .as_ref()
            .or(old.as_ref())
            .map(|i| i["sk"]["S"].clone())
            .unwrap();
        let mut dynamodb = json!({
            "ApproximateCreationDateTime": 1760886202,
            "Keys": { "pk": { "S": "NOTE#user-1" }, "sk": { "S": path } },
            "SequenceNumber": seq,
            "SizeBytes": 321,
            "StreamViewType": "NEW_AND_OLD_IMAGES"
        });
        if let Some(o) = old {
            dynamodb["OldImage"] = o;
        }
        if let Some(n) = new {
            dynamodb["NewImage"] = n;
        }
        json!({
            "eventID": format!("id-{seq}"),
            "eventName": name,
            "eventVersion": "1.1",
            "eventSource": "aws:dynamodb",
            "awsRegion": "us-west-2",
            "dynamodb": dynamodb,
            "eventSourceARN": "arn:aws:dynamodb:us-west-2:123456789012:table/hades_journal/stream/2026-10-20T00:00:00.000"
        })
    }

    fn stream(records: Vec<Value>) -> StreamEvent {
        match parse_invocation(json!({ "Records": records })).unwrap() {
            Invocation::Stream(e) => e,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dispatch() {
        assert!(matches!(
            parse_invocation(json!({ "backfill": true })).unwrap(),
            Invocation::Backfill
        ));
        assert!(parse_invocation(json!({ "backfill": false })).is_err());
        assert!(parse_invocation(json!({ "backfill": "yes" })).is_err());
        assert!(parse_invocation(json!({})).is_err());
        assert!(parse_invocation(json!({ "Records": "nope" })).is_err());
        let e = stream(vec![record(
            "INSERT",
            "1",
            None,
            Some(image("a.md", HASH_A, None, None)),
        )]);
        assert_eq!(e.records.len(), 1);
    }

    #[test]
    fn insert_needs_indexing() {
        let e = stream(vec![record(
            "INSERT",
            "100",
            None,
            Some(image("Projects/Design #1.md", HASH_A, None, None)),
        )]);
        match change_from_record(&e.records[0]).unwrap() {
            Change::Reindex {
                sub,
                record,
                stale_count,
            } => {
                assert_eq!(sub, "user-1");
                assert_eq!(record.meta.path, "Projects/Design #1.md");
                assert_eq!(record.meta.project.as_deref(), Some("hades"));
                assert_eq!(record.s3_version_id, "v1");
                assert_eq!(record.meta.content_hash, HASH_A);
                assert_eq!(stale_count, 0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn modify_after_edit_keeps_old_chunk_count() {
        // The API's update leaves indexedHash/chunkCount alone, so the new image still has them.
        let e = stream(vec![record(
            "MODIFY",
            "101",
            Some(image("a.md", HASH_A, Some(HASH_A), Some(5))),
            Some(image("a.md", HASH_B, Some(HASH_A), Some(5))),
        )]);
        match change_from_record(&e.records[0]).unwrap() {
            Change::Reindex {
                record,
                stale_count,
                ..
            } => {
                assert_eq!(record.meta.content_hash, HASH_B);
                assert!(!record.meta.indexed);
                assert_eq!(stale_count, 5);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn own_write_back_is_skipped() {
        let e = stream(vec![record(
            "MODIFY",
            "102",
            Some(image("a.md", HASH_B, Some(HASH_A), Some(5))),
            Some(image("a.md", HASH_B, Some(HASH_B), Some(3))),
        )]);
        assert_eq!(
            change_from_record(&e.records[0]).unwrap(),
            Change::Skip("already indexed")
        );
    }

    #[test]
    fn remove_uses_old_chunk_count() {
        let e = stream(vec![
            record(
                "REMOVE",
                "103",
                Some(image("a.md", HASH_A, Some(HASH_A), Some(4))),
                None,
            ),
            record(
                "REMOVE",
                "104",
                Some(image("b.md", HASH_A, None, None)),
                None,
            ),
        ]);
        assert_eq!(
            change_from_record(&e.records[0]).unwrap(),
            Change::Remove {
                sub: "user-1".into(),
                path: "a.md".into(),
                chunk_count: 4
            }
        );
        assert_eq!(
            change_from_record(&e.records[1]).unwrap(),
            Change::Remove {
                sub: "user-1".into(),
                path: "b.md".into(),
                chunk_count: 0
            }
        );
    }

    #[test]
    fn non_note_items_are_skipped() {
        let mut r = record(
            "INSERT",
            "105",
            None,
            Some(image("a.md", HASH_A, None, None)),
        );
        r["dynamodb"]["Keys"]["pk"]["S"] = json!("JOURNAL#user-1");
        r["dynamodb"]["NewImage"]["pk"]["S"] = json!("JOURNAL#user-1");
        let e = stream(vec![r]);
        assert_eq!(
            change_from_record(&e.records[0]).unwrap(),
            Change::Skip("not a note")
        );
    }

    #[test]
    fn malformed_records_are_errors() {
        // Missing NewImage on INSERT.
        let mut r = record(
            "INSERT",
            "106",
            None,
            Some(image("a.md", HASH_A, None, None)),
        );
        r["dynamodb"].as_object_mut().unwrap().remove("NewImage");
        assert!(change_from_record(&stream(vec![r]).records[0]).is_err());
        // Image missing a required attribute.
        let mut img = image("a.md", HASH_A, None, None);
        img.as_object_mut().unwrap().remove("s3VersionId");
        let r = record("MODIFY", "107", None, Some(img));
        assert!(change_from_record(&stream(vec![r]).records[0]).is_err());
        assert!(change_from_images("INSERT", None, None, None).is_err());
        let keys: Item = HashMap::from([
            ("pk".into(), AttributeValue::S("NOTE#u".into())),
            ("sk".into(), AttributeValue::S("a.md".into())),
        ]);
        assert!(change_from_images("TRUNCATE", Some(&keys), None, None).is_err());
    }

    #[test]
    fn partial_failure_response_shape() {
        let e = stream(vec![
            record("INSERT", "1", None, Some(image("a.md", HASH_A, None, None))),
            record("INSERT", "2", None, Some(image("b.md", HASH_A, None, None))),
            record("INSERT", "3", None, Some(image("c.md", HASH_A, None, None))),
        ]);
        let resp = batch_failures(&e.records[1..]);
        assert_eq!(
            serde_json::to_value(resp).unwrap(),
            json!({ "batchItemFailures": [
                { "itemIdentifier": "2" }, { "itemIdentifier": "3" }
            ] })
        );
        assert_eq!(
            serde_json::to_value(DynamoDbEventResponse::default()).unwrap(),
            json!({ "batchItemFailures": [] })
        );
    }

    #[test]
    fn backfill_report_shape() {
        let r = BackfillReport {
            scanned: 3,
            up_to_date: 1,
            reindexed: 2,
            superseded: 0,
            failed: 0,
        };
        assert_eq!(
            serde_json::to_value(r).unwrap(),
            json!({ "scanned": 3, "upToDate": 1, "reindexed": 2, "superseded": 0, "failed": 0 })
        );
    }
}
