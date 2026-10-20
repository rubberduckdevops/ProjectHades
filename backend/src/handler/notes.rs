use std::sync::Arc;

use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::StatusCode,
};

use crate::{
    AppState,
    auth::AuthUser,
    error::ApiError,
    models::{DeleteNoteQuery, ListNotesQuery, MoveNoteRequest, Note, NoteMeta, PutNoteRequest},
    notes::{MAX_CONTENT_BYTES, MAX_PATH_BYTES, is_valid_hash, validate_path},
    repo::notes::NotesRepo,
};

/// Request body limit for `PUT /api/notes/{path}`. Content is capped at 1 MB, but JSON
/// escaping can inflate it up to 6x (`\u0000`), so the transport limit is higher than axum's
/// 2 MB default. Anything larger is rejected by the `Json` extractor and mapped to a JSON 400.
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

fn repo(state: &AppState) -> NotesRepo<'_> {
    NotesRepo::new(
        &state.ddb,
        &state.s3,
        &state.table_name,
        &state.notes_bucket,
    )
}

/// Validate a note path, naming the field in the error.
fn check_path(path: &str, what: &str) -> Result<(), ApiError> {
    validate_path(path).map_err(|reason| ApiError::bad_request(format!("invalid {what}: {reason}")))
}

/// The `{*path}` capture. Axum percent-decodes it exactly once (`My%20Ideas` -> `My Ideas`,
/// `%2520` -> `%20`); it is not decoded again here.
fn note_path(path: Result<Path<String>, PathRejection>) -> Result<String, ApiError> {
    let Path(path) = path.map_err(|e| ApiError::bad_request(e.body_text()))?;
    check_path(&path, "path")?;
    Ok(path)
}

/// A client-supplied `baseHash`: 64 hex digits, normalized to lowercase.
fn base_hash(h: &str) -> Result<String, ApiError> {
    if !is_valid_hash(h) {
        return Err(ApiError::bad_request(
            "baseHash must be a 64-character hex sha256",
        ));
    }
    Ok(h.to_ascii_lowercase())
}

/// `GET /api/notes?prefix=`
pub async fn list_notes(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    query: Result<Query<ListNotesQuery>, QueryRejection>,
) -> Result<Json<Vec<NoteMeta>>, ApiError> {
    let Query(q) = query.map_err(|e| ApiError::bad_request(e.body_text()))?;
    if q.prefix.as_ref().is_some_and(|p| p.len() > MAX_PATH_BYTES) {
        return Err(ApiError::bad_request(format!(
            "prefix must be at most {MAX_PATH_BYTES} bytes"
        )));
    }
    Ok(Json(
        repo(&state).list(&user.sub, q.prefix.as_deref()).await?,
    ))
}

/// `GET /api/notes/{path...}`
pub async fn get_note(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<Note>, ApiError> {
    let path = note_path(path)?;
    repo(&state)
        .get(&user.sub, &path)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("note not found"))
}

/// `PUT /api/notes/{path...}`: create (no `baseHash`, 201) or update (`baseHash`, 200).
pub async fn put_note(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
    input: Result<Json<PutNoteRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<NoteMeta>), ApiError> {
    let path = note_path(path)?;
    let Json(input) = input.map_err(|e| ApiError::bad_request(e.body_text()))?;
    if input.content.len() > MAX_CONTENT_BYTES {
        return Err(ApiError::bad_request(format!(
            "content must be at most {MAX_CONTENT_BYTES} bytes"
        )));
    }
    let base = input.base_hash.as_deref().map(base_hash).transpose()?;

    let repo = repo(&state);
    match base {
        None => {
            let meta = repo.create(&user.sub, &path, &input.content).await?;
            Ok((StatusCode::CREATED, Json(meta)))
        }
        Some(base) => {
            let meta = repo.update(&user.sub, &path, &input.content, &base).await?;
            Ok((StatusCode::OK, Json(meta)))
        }
    }
}

/// `DELETE /api/notes/{path...}?baseHash=`
pub async fn delete_note(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<DeleteNoteQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    let path = note_path(path)?;
    let Query(q) = query.map_err(|e| ApiError::bad_request(e.body_text()))?;
    let base = q
        .base_hash
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("baseHash is required"))
        .and_then(base_hash)?;
    repo(&state).delete(&user.sub, &path, &base).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/notes/move`
pub async fn move_note(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    input: Result<Json<MoveNoteRequest>, JsonRejection>,
) -> Result<Json<NoteMeta>, ApiError> {
    let Json(input) = input.map_err(|e| ApiError::bad_request(e.body_text()))?;
    check_path(&input.from, "from")?;
    check_path(&input.to, "to")?;
    if input.from == input.to {
        return Err(ApiError::bad_request("from and to must differ"));
    }
    let base = base_hash(&input.base_hash)?;
    let meta = repo(&state)
        .move_note(&user.sub, &input.from, &input.to, &base)
        .await?;
    Ok(Json(meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_hash_validation() {
        let h = "AB".repeat(32);
        assert_eq!(base_hash(&h).unwrap(), "ab".repeat(32));
        assert!(base_hash("abc").is_err());
        assert!(base_hash(&"z".repeat(64)).is_err());
    }

    #[test]
    fn check_path_names_field() {
        match check_path("noext", "to") {
            Err(ApiError::BadRequest(m)) => assert!(m.starts_with("invalid to:"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
}
