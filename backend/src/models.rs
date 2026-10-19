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
