use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

use crate::{
    AppState, auth::AuthUser, error::ApiError, handler::journal::normalize_project,
    models::Project, repo::projects::ProjectRepo,
};

/// `GET /api/projects`
pub async fn list_projects(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Project>>, ApiError> {
    let projects = ProjectRepo::new(&state.ddb, &state.table_name)
        .list(&user.sub)
        .await?;
    Ok(Json(projects))
}

/// `DELETE /api/projects/{name}`
pub async fn delete_project(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let name = normalize_project(Some(name))?
        .ok_or_else(|| ApiError::bad_request("name must not be blank"))?;
    if ProjectRepo::new(&state.ddb, &state.table_name)
        .delete(&user.sub, &name)
        .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("not found"))
    }
}
