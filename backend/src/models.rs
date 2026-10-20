use serde::{Deserialize, Serialize};

/// Request body for `PUT /api/journal`.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateOrUpdateJournalEntry {
    pub body: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
}

/// A journal entry as returned by the API. `id` and `timestamp` both equal the sort key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: String,
    pub timestamp: String,
    /// Free-text project; `None` means a general entry.
    pub project: Option<String>,
    pub body: String,
}

impl JournalEntry {
    pub fn new(sk: String, project: Option<String>, body: String) -> Self {
        Self {
            id: sk.clone(),
            timestamp: sk,
            project,
            body,
        }
    }
}

/// Query string for `GET /api/journal`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListQuery {
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Note metadata as returned by the notes API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteMeta {
    pub path: String,
    pub title: String,
    /// Always serialized; `None` = no project.
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub content_hash: String,
    pub size: u64,
    pub created_at: String,
    pub updated_at: String,
    /// Whether the search index reflects the current content (`indexedHash == contentHash`).
    pub indexed: bool,
}

/// A note with its content (`GET /api/notes/{path}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    #[serde(flatten)]
    pub meta: NoteMeta,
    pub content: String,
}

/// Request body for `PUT /api/notes/{path}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutNoteRequest {
    pub content: String,
    #[serde(default)]
    pub base_hash: Option<String>,
}

/// Request body for `POST /api/notes/move`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveNoteRequest {
    pub from: String,
    pub to: String,
    pub base_hash: String,
}

/// Query string for `GET /api/notes`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListNotesQuery {
    pub prefix: Option<String>,
}

/// Query string for `DELETE /api/notes/{path}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteNoteQuery {
    pub base_hash: Option<String>,
}

/// Query string for `GET /api/search`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub project: Option<String>,
    /// Parsed by the handler so a bad value gets a clear 400.
    pub limit: Option<String>,
}

/// A todo as returned by the API. `id` equals `created_at` (the sort key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    pub id: String,
    pub title: String,
    /// Always serialized; `None` = no project.
    pub project: Option<String>,
    /// Due date as `YYYY-MM-DD`; always serialized.
    pub due: Option<String>,
    pub done: bool,
    pub created_at: String,
    /// Set while `done` is true; always serialized.
    pub completed_at: Option<String>,
    pub updated_at: String,
}

/// Request body for `POST /api/todos`.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTodo {
    pub title: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub due: Option<String>,
}

/// Request body for `PATCH /api/todos/{id}`. The outer `Option` is "field present"; for
/// `project` and `due` the inner `None` (JSON `null`) clears the field.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PatchTodo {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub project: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub due: Option<Option<String>>,
    #[serde(default)]
    pub done: Option<bool>,
}

/// Distinguishes a present `null` (`Some(None)`) from a missing field (`None`, via `default`).
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}
