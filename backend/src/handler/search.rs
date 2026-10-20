use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
};

use crate::{
    AppState,
    auth::AuthUser,
    error::ApiError,
    models::SearchQuery,
    repo::notes::NotesRepo,
    search::{
        DEFAULT_LIMIT, MAX_LIMIT, MAX_QUERY_CHARS, SearchResult, TOP_K, best_hits, merge_results,
        search_filter,
    },
};

/// Validated search parameters.
#[derive(Debug, PartialEq, Eq)]
pub struct SearchParams {
    pub q: String,
    pub project: Option<String>,
    pub limit: usize,
}

/// `q`: 1–500 characters after trimming. `project`: trimmed, blank = none. `limit`: 1–50.
pub fn validate(q: SearchQuery) -> Result<SearchParams, ApiError> {
    let text = q.q.as_deref().map(str::trim).unwrap_or_default();
    if text.is_empty() {
        return Err(ApiError::bad_request("q is required"));
    }
    if text.chars().count() > MAX_QUERY_CHARS {
        return Err(ApiError::bad_request(format!(
            "q must be at most {MAX_QUERY_CHARS} characters"
        )));
    }
    let limit = match q.limit.as_deref().map(str::trim) {
        None => DEFAULT_LIMIT,
        Some(l) => l
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .ok_or_else(|| {
                ApiError::bad_request(format!("limit must be an integer from 1 to {MAX_LIMIT}"))
            })?,
    };
    let project = q
        .project
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty());
    Ok(SearchParams {
        q: text.to_owned(),
        project,
        limit,
    })
}

/// `GET /api/search?q=&project=&limit=`
pub async fn search(
    user: AuthUser,
    State(state): State<Arc<AppState>>,
    query: Result<Query<SearchQuery>, QueryRejection>,
) -> Result<Json<Vec<SearchResult>>, ApiError> {
    let Query(q) = query.map_err(|e| ApiError::bad_request(e.body_text()))?;
    let params = validate(q)?;
    let filter = search_filter(&user.sub, params.project.as_deref());

    let semantic = async {
        let vector = state.embedder.embed(&params.q).await?;
        state.vectors.query(vector, TOP_K, filter).await
    };
    let repo = NotesRepo::new(
        &state.ddb,
        &state.s3,
        &state.table_name,
        &state.notes_bucket,
    );
    let (matches, notes) = tokio::try_join!(semantic, repo.list(&user.sub, None))?;

    Ok(Json(merge_results(
        &params.q,
        params.project.as_deref(),
        &notes,
        best_hits(&user.sub, &matches),
        params.limit,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(
        q: Option<&str>,
        project: Option<&str>,
        limit: Option<&str>,
    ) -> Result<SearchParams, ApiError> {
        validate(SearchQuery {
            q: q.map(str::to_owned),
            project: project.map(str::to_owned),
            limit: limit.map(str::to_owned),
        })
    }

    #[test]
    fn validation() {
        assert_eq!(
            v(Some("  hello "), Some(" hades "), None).unwrap(),
            SearchParams {
                q: "hello".into(),
                project: Some("hades".into()),
                limit: 10
            }
        );
        assert_eq!(v(Some("x"), Some("  "), Some("50")).unwrap().project, None);
        assert_eq!(v(Some("x"), None, Some("1")).unwrap().limit, 1);
        assert!(v(Some(&"é".repeat(500)), None, None).is_ok());
        for bad in [
            v(None, None, None),
            v(Some("   "), None, None),
            v(Some(&"é".repeat(501)), None, None),
            v(Some("x"), None, Some("0")),
            v(Some("x"), None, Some("51")),
            v(Some("x"), None, Some("-1")),
            v(Some("x"), None, Some("ten")),
            v(Some("x"), None, Some("")),
        ] {
            assert!(matches!(bad, Err(ApiError::BadRequest(_))), "{bad:?}");
        }
    }
}
