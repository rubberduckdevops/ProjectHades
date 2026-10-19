//! DynamoDB access for journal entries.
//!
//! Item layout: `pk` (S) = `JOURNAL#<sub>`, `sk` (S) = RFC 3339 UTC with millis and `Z`,
//! `body` (S) = entry text.

use std::collections::HashMap;

use aws_sdk_dynamodb::{
    Client,
    types::{AttributeValue, ReturnValue},
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};

use crate::{error::ApiError, models::JournalEntry};

pub const PK_PREFIX: &str = "JOURNAL#";

/// Partition key for a user.
pub fn pk_for(sub: &str) -> String {
    format!("{PK_PREFIX}{sub}")
}

/// Sort-key format: `2026-10-19T14:03:22.123Z`.
pub fn format_sk(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Parse any RFC 3339 timestamp and convert it to UTC.
pub fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// Parse an RFC 3339 timestamp and normalize it to the sort-key format
/// (UTC, millisecond precision; finer precision is truncated).
pub fn normalize_timestamp(s: &str) -> Option<String> {
    parse_timestamp(s).map(format_sk)
}

/// Inclusive `[00:00:00.000, 23:59:59.999]` range of the UTC day containing `now`.
pub fn utc_day_range(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc();
    let end = start + Duration::days(1) - Duration::milliseconds(1);
    (start, end)
}

pub struct JournalRepo<'a> {
    client: &'a Client,
    table: &'a str,
}

impl<'a> JournalRepo<'a> {
    pub fn new(client: &'a Client, table: &'a str) -> Self {
        Self { client, table }
    }

    fn item(
        sub: &str,
        sk: &str,
        project: Option<&str>,
        body: &str,
    ) -> HashMap<String, AttributeValue> {
        let mut item = HashMap::from([
            ("pk".to_owned(), AttributeValue::S(pk_for(sub))),
            ("sk".to_owned(), AttributeValue::S(sk.to_owned())),
            ("body".to_owned(), AttributeValue::S(body.to_owned())),
        ]);
        if let Some(p) = project {
            item.insert("project".to_owned(), AttributeValue::S(p.to_owned()));
        }
        item
    }

    /// Insert a new entry at `now`. Never overwrites: on a sort-key collision it retries
    /// once with a fresh timestamp.
    pub async fn put_new(
        &self,
        sub: &str,
        project: Option<&str>,
        body: &str,
    ) -> Result<JournalEntry, ApiError> {
        let mut attempt = 0;
        loop {
            let sk = format_sk(Utc::now());
            let res = self
                .client
                .put_item()
                .table_name(self.table)
                .set_item(Some(Self::item(sub, &sk, project, body)))
                .condition_expression("attribute_not_exists(sk)")
                .send()
                .await;
            match res {
                Ok(_) => {
                    return Ok(JournalEntry::new(
                        sk,
                        project.map(str::to_owned),
                        body.to_owned(),
                    ));
                }
                Err(e)
                    if attempt == 0
                        && e.as_service_error()
                            .is_some_and(|se| se.is_conditional_check_failed_exception()) =>
                {
                    attempt += 1;
                    // Make sure the next `now` lands on a different millisecond.
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
                Err(e) => {
                    return Err(ApiError::internal(format!(
                        "PutItem (new) failed: {}",
                        aws_sdk_dynamodb::error::DisplayErrorContext(&e)
                    )));
                }
            }
        }
    }

    /// Put an entry at an explicit, already-normalized sort key.
    /// Returns the entry and whether it was newly created (`false` = replaced).
    pub async fn upsert(
        &self,
        sub: &str,
        sk: &str,
        project: Option<&str>,
        body: &str,
    ) -> Result<(JournalEntry, bool), ApiError> {
        let out = self
            .client
            .put_item()
            .table_name(self.table)
            .set_item(Some(Self::item(sub, sk, project, body)))
            .return_values(ReturnValue::AllOld)
            .send()
            .await
            .map_err(|e| {
                ApiError::internal(format!(
                    "PutItem (upsert) failed: {}",
                    aws_sdk_dynamodb::error::DisplayErrorContext(&e)
                ))
            })?;
        let created = out.attributes().is_none_or(|a| a.is_empty());
        Ok((
            JournalEntry::new(sk.to_owned(), project.map(str::to_owned), body.to_owned()),
            created,
        ))
    }

    /// All entries with `from <= sk <= to`, ascending. Bounds must already be normalized.
    pub async fn list(
        &self,
        sub: &str,
        from: &str,
        to: &str,
    ) -> Result<Vec<JournalEntry>, ApiError> {
        let mut entries = Vec::new();
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;
        loop {
            let out = self
                .client
                .query()
                .table_name(self.table)
                .key_condition_expression("pk = :pk AND sk BETWEEN :from AND :to")
                .expression_attribute_values(":pk", AttributeValue::S(pk_for(sub)))
                .expression_attribute_values(":from", AttributeValue::S(from.to_owned()))
                .expression_attribute_values(":to", AttributeValue::S(to.to_owned()))
                .scan_index_forward(true)
                .set_exclusive_start_key(start_key.take())
                .send()
                .await
                .map_err(|e| {
                    ApiError::internal(format!(
                        "Query failed: {}",
                        aws_sdk_dynamodb::error::DisplayErrorContext(&e)
                    ))
                })?;
            for item in out.items() {
                entries.push(entry_from_item(item)?);
            }
            match out.last_evaluated_key() {
                Some(k) if !k.is_empty() => start_key = Some(k.clone()),
                _ => break,
            }
        }
        Ok(entries)
    }

    /// A single entry by normalized sort key.
    pub async fn get(&self, sub: &str, sk: &str) -> Result<Option<JournalEntry>, ApiError> {
        let out = self
            .client
            .get_item()
            .table_name(self.table)
            .key("pk", AttributeValue::S(pk_for(sub)))
            .key("sk", AttributeValue::S(sk.to_owned()))
            .send()
            .await
            .map_err(|e| {
                ApiError::internal(format!(
                    "GetItem failed: {}",
                    aws_sdk_dynamodb::error::DisplayErrorContext(&e)
                ))
            })?;
        out.item().map(entry_from_item).transpose()
    }
}

fn entry_from_item(item: &HashMap<String, AttributeValue>) -> Result<JournalEntry, ApiError> {
    let s = |name: &str| {
        item.get(name)
            .and_then(|v| v.as_s().ok())
            .cloned()
            .ok_or_else(|| ApiError::internal(format!("item missing string attribute {name}")))
    };
    // Optional: entries saved before projects existed have no `project` attribute.
    let project = item.get("project").and_then(|v| v.as_s().ok()).cloned();
    Ok(JournalEntry::new(s("sk")?, project, s("body")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn pk_builder() {
        assert_eq!(pk_for("abc-123"), "JOURNAL#abc-123");
    }

    #[test]
    fn sk_format_has_millis_and_z() {
        let t =
            Utc.with_ymd_and_hms(2026, 10, 19, 14, 3, 22).unwrap() + Duration::milliseconds(123);
        assert_eq!(format_sk(t), "2026-10-19T14:03:22.123Z");
        let whole = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        assert_eq!(format_sk(whole), "2026-01-02T03:04:05.000Z");
    }

    #[test]
    fn normalizes_timestamps() {
        assert_eq!(
            normalize_timestamp("2026-10-19T14:03:22.123Z").as_deref(),
            Some("2026-10-19T14:03:22.123Z")
        );
        assert_eq!(
            normalize_timestamp("2026-10-19T14:03:22Z").as_deref(),
            Some("2026-10-19T14:03:22.000Z")
        );
        assert_eq!(
            normalize_timestamp("2026-10-19T16:03:22.5+02:00").as_deref(),
            Some("2026-10-19T14:03:22.500Z")
        );
        assert_eq!(
            normalize_timestamp("2026-10-19T14:03:22.123456789Z").as_deref(),
            Some("2026-10-19T14:03:22.123Z")
        );
        // JS `new Date(y, m, d).toISOString()` output for a US-Pacific local midnight.
        assert_eq!(
            normalize_timestamp("2026-10-19T07:00:00.000Z").as_deref(),
            Some("2026-10-19T07:00:00.000Z")
        );
    }

    #[test]
    fn rejects_bad_timestamps() {
        for s in [
            "",
            "nope",
            "2026-10-19",
            "2026-10-19T14:03:22",
            "1729346602",
        ] {
            assert_eq!(normalize_timestamp(s), None, "{s}");
        }
    }

    #[test]
    fn default_day_range() {
        let now = Utc.with_ymd_and_hms(2026, 10, 19, 14, 3, 22).unwrap();
        let (from, to) = utc_day_range(now);
        assert_eq!(format_sk(from), "2026-10-19T00:00:00.000Z");
        assert_eq!(format_sk(to), "2026-10-19T23:59:59.999Z");
    }

    #[test]
    fn entry_from_item_reads_sk_and_body() {
        let item = JournalRepo::item("u", "2026-10-19T14:03:22.123Z", None, "hi");
        let e = entry_from_item(&item).unwrap();
        assert_eq!(e.id, "2026-10-19T14:03:22.123Z");
        assert_eq!(e.timestamp, e.id);
        assert_eq!(e.body, "hi");
        assert_eq!(e.project, None);
        assert!(!item.contains_key("project"));
        assert_eq!(item["pk"].as_s().unwrap(), "JOURNAL#u");
    }

    #[test]
    fn entry_from_item_reads_optional_project() {
        let item = JournalRepo::item("u", "2026-10-19T14:03:22.123Z", Some("hades"), "hi");
        assert_eq!(item["project"].as_s().unwrap(), "hades");
        let e = entry_from_item(&item).unwrap();
        assert_eq!(e.project.as_deref(), Some("hades"));
    }
}
