use std::sync::Arc;

use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::StatusCode,
};
use chrono::Utc;

use crate::{
    AppState,
    auth::AuthUser,
    error::ApiError,
    models::{CreateOrUpdateJournalEntry, JournalEntry, ListQuery},
    repo::journal::{JournalRepo, format_sk, normalize_timestamp, utc_day_range},
};

/// Maximum entry body size in bytes (10 KB).
pub const MAX_BODY_BYTES: usize = 10 * 1024;

/// Body must be non-empty after trimming and at most `MAX_BODY_BYTES` (UTF-8 bytes).
/// The body is stored as sent (not trimmed).
pub fn validate_body(body: &str) -> Result<(), ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request("body must not be empty"));
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(ApiError::bad_request(format!(
            "body must be at most {MAX_BODY_BYTES} bytes"
        )));
    }
    Ok(())
}

fn normalize_or_400(value: &str, what: &str) -> Result<String, ApiError> {
    normalize_timestamp(value)
        .ok_or_else(|| ApiError::bad_request(format!("{what} must be an RFC 3339 timestamp")))
}

/// Resolve the inclusive `(from, to)` sort-key range, defaulting to the current UTC day.
pub fn resolve_range(q: &ListQuery) -> Result<(String, String), ApiError> {
    let (day_start, day_end) = utc_day_range(Utc::now());
    let from = match q.from.as_deref() {
        Some(s) => normalize_or_400(s, "from")?,
        None => format_sk(day_start),
    };
    let to = match q.to.as_deref() {
        Some(s) => normalize_or_400(s, "to")?,
        None => format_sk(day_end),
    };
    if from > to {
        return Err(ApiError::bad_request("from must not be after to"));
    }
    Ok((from, to))
}

/// `GET /api/journal?from=&to=`
pub async fn list_journal_entries(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Vec<JournalEntry>>, ApiError> {
    let Query(q) = query.map_err(|e| ApiError::bad_request(e.body_text()))?;
    let (from, to) = resolve_range(&q)?;
    let entries = JournalRepo::new(&state.ddb, &state.table_name)
        .list(&user.sub, &from, &to)
        .await?;
    Ok(Json(entries))
}

/// `GET /api/journal/{timestamp}`
pub async fn get_journal_entry(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(timestamp): Path<String>,
) -> Result<Json<JournalEntry>, ApiError> {
    let sk = normalize_or_400(&timestamp, "timestamp")?;
    JournalRepo::new(&state.ddb, &state.table_name)
        .get(&user.sub, &sk)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("not found".into()))
}

/// `PUT /api/journal` — insert (no id, 201) or upsert by id (200 replaced / 201 created).
pub async fn put_journal_entry(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    input: Result<Json<CreateOrUpdateJournalEntry>, JsonRejection>,
) -> Result<(StatusCode, Json<JournalEntry>), ApiError> {
    let Json(input) = input.map_err(|e| ApiError::bad_request(e.body_text()))?;
    validate_body(&input.body)?;
    let sk = input
        .id
        .as_deref()
        .map(|id| normalize_or_400(id, "id"))
        .transpose()?;

    let repo = JournalRepo::new(&state.ddb, &state.table_name);
    match sk {
        None => {
            let entry = repo.put_new(&user.sub, &input.body).await?;
            Ok((StatusCode::CREATED, Json(entry)))
        }
        Some(sk) => {
            let (entry, created) = repo.upsert(&user.sub, &sk, &input.body).await?;
            let status = if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            Ok((status, Json(entry)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_validation() {
        assert!(validate_body("hello").is_ok());
        assert!(validate_body("  padded  ").is_ok());
        assert!(validate_body("").is_err());
        assert!(validate_body(" \n\t ").is_err());
        assert!(validate_body(&"a".repeat(MAX_BODY_BYTES)).is_ok());
        assert!(validate_body(&"a".repeat(MAX_BODY_BYTES + 1)).is_err());
        // Limit is in bytes: 3-byte chars.
        assert!(validate_body(&"€".repeat(MAX_BODY_BYTES / 3 + 1)).is_err());
    }

    #[test]
    fn range_defaults_to_today_utc() {
        let (from, to) = resolve_range(&ListQuery::default()).unwrap();
        let today = Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(from, format!("{today}T00:00:00.000Z"));
        assert_eq!(to, format!("{today}T23:59:59.999Z"));
    }

    #[test]
    fn range_normalizes_and_validates() {
        let q = ListQuery {
            from: Some("2026-10-19T07:00:00Z".into()),
            to: Some("2026-10-20T06:59:59.999+00:00".into()),
        };
        assert_eq!(
            resolve_range(&q).unwrap(),
            (
                "2026-10-19T07:00:00.000Z".to_owned(),
                "2026-10-20T06:59:59.999Z".to_owned()
            )
        );
        let bad = ListQuery {
            from: Some("yesterday".into()),
            to: None,
        };
        assert!(matches!(resolve_range(&bad), Err(ApiError::BadRequest(_))));
        let inverted = ListQuery {
            from: Some("2026-10-20T00:00:00Z".into()),
            to: Some("2026-10-19T00:00:00Z".into()),
        };
        assert!(matches!(
            resolve_range(&inverted),
            Err(ApiError::BadRequest(_))
        ));
    }
}
