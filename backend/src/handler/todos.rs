use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};
use chrono::NaiveDate;

use crate::{
    AppState,
    auth::AuthUser,
    error::ApiError,
    handler::journal::normalize_project,
    models::{CreateTodo, PatchTodo, Todo},
    repo::{
        journal::normalize_timestamp,
        todos::{TodoRepo, TodoUpdate},
    },
};

/// Maximum title size in bytes.
pub const MAX_TITLE_BYTES: usize = 500;

/// Trimmed title; must be non-empty and at most `MAX_TITLE_BYTES` (UTF-8 bytes).
pub fn normalize_title(title: &str) -> Result<String, ApiError> {
    let t = title.trim();
    if t.is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    if t.len() > MAX_TITLE_BYTES {
        return Err(ApiError::bad_request(format!(
            "title must be at most {MAX_TITLE_BYTES} bytes"
        )));
    }
    Ok(t.to_owned())
}

/// A due date must be a real calendar date written as `YYYY-MM-DD`.
pub fn validate_due(due: &str) -> Result<String, ApiError> {
    let d = due.trim();
    match NaiveDate::parse_from_str(d, "%Y-%m-%d") {
        // Round-trip so `2026-1-5` (accepted by chrono) isn't stored in a second format.
        Ok(date) if date.format("%Y-%m-%d").to_string() == d => Ok(d.to_owned()),
        _ => Err(ApiError::bad_request(
            "due must be a date in YYYY-MM-DD format",
        )),
    }
}

fn id_or_400(id: &str) -> Result<String, ApiError> {
    normalize_timestamp(id).ok_or_else(|| ApiError::bad_request("id must be an RFC 3339 timestamp"))
}

/// Validate a patch body into a repo update.
pub fn validate_patch(patch: PatchTodo) -> Result<TodoUpdate, ApiError> {
    Ok(TodoUpdate {
        title: patch.title.as_deref().map(normalize_title).transpose()?,
        // A blank project normalizes to `None`, which clears it, same as `null`.
        project: patch.project.map(normalize_project).transpose()?,
        due: patch
            .due
            .map(|d| d.as_deref().map(validate_due).transpose())
            .transpose()?,
        done: patch.done,
    })
}

/// `GET /api/todos`
pub async fn list_todos(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Todo>>, ApiError> {
    let todos = TodoRepo::new(&state.ddb, &state.table_name)
        .list(&user.sub)
        .await?;
    Ok(Json(todos))
}

/// `POST /api/todos`
pub async fn create_todo(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    input: Result<Json<CreateTodo>, JsonRejection>,
) -> Result<(StatusCode, Json<Todo>), ApiError> {
    let Json(input) = input.map_err(|e| ApiError::bad_request(e.body_text()))?;
    let title = normalize_title(&input.title)?;
    let project = normalize_project(input.project)?;
    let due = input.due.as_deref().map(validate_due).transpose()?;
    let todo = TodoRepo::new(&state.ddb, &state.table_name)
        .create(&user.sub, &title, project.as_deref(), due.as_deref())
        .await?;
    Ok((StatusCode::CREATED, Json(todo)))
}

/// `PATCH /api/todos/{id}`
pub async fn patch_todo(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    input: Result<Json<PatchTodo>, JsonRejection>,
) -> Result<Json<Todo>, ApiError> {
    let id = id_or_400(&id)?;
    let Json(input) = input.map_err(|e| ApiError::bad_request(e.body_text()))?;
    let update = validate_patch(input)?;
    TodoRepo::new(&state.ddb, &state.table_name)
        .update(&user.sub, &id, &update)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("not found"))
}

/// `DELETE /api/todos/{id}`
pub async fn delete_todo(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = id_or_400(&id)?;
    if TodoRepo::new(&state.ddb, &state.table_name)
        .delete(&user.sub, &id)
        .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("not found"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_validation() {
        assert_eq!(normalize_title("  Buy milk ").unwrap(), "Buy milk");
        assert!(normalize_title("").is_err());
        assert!(normalize_title(" \n ").is_err());
        assert!(normalize_title(&"a".repeat(MAX_TITLE_BYTES)).is_ok());
        assert!(normalize_title(&"a".repeat(MAX_TITLE_BYTES + 1)).is_err());
    }

    #[test]
    fn due_validation() {
        assert_eq!(validate_due("2026-11-01").unwrap(), "2026-11-01");
        assert_eq!(validate_due(" 2028-02-29 ").unwrap(), "2028-02-29");
        for bad in [
            "",
            "tomorrow",
            "2026-02-30",
            "2026-1-5",
            "2026-11-01T00:00:00Z",
            "11/01/2026",
        ] {
            assert!(validate_due(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn patch_parsing_distinguishes_null_from_missing() {
        let p: PatchTodo = serde_json::from_str(r#"{"project": null, "done": true}"#).unwrap();
        assert_eq!(p.project, Some(None));
        assert_eq!(p.due, None);
        let u = validate_patch(p).unwrap();
        assert_eq!(
            u,
            TodoUpdate {
                project: Some(None),
                done: Some(true),
                ..Default::default()
            }
        );
    }

    #[test]
    fn patch_validation() {
        let p: PatchTodo =
            serde_json::from_str(r#"{"title": " T ", "project": "  ", "due": "2026-12-01"}"#)
                .unwrap();
        let u = validate_patch(p).unwrap();
        assert_eq!(u.title.as_deref(), Some("T"));
        assert_eq!(u.project, Some(None), "blank project clears");
        assert_eq!(u.due, Some(Some("2026-12-01".to_owned())));

        for bad in [
            r#"{"title": ""}"#,
            r#"{"due": "soon"}"#,
            r#"{"project": "PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP"}"#,
        ] {
            let p: PatchTodo = serde_json::from_str(bad).unwrap();
            assert!(
                matches!(validate_patch(p), Err(ApiError::BadRequest(_))),
                "{bad}"
            );
        }
    }
}
