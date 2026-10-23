//! DynamoDB access for the project registry: every project name the user has used, so the
//! suggestions don't depend on what happens to be loaded on screen.
//!
//! Item layout under `pk` (S) = `PROJECT#<sub>`:
//! - `sk` = `NAME#<lowercased name>`: `name` (S, as last written) and `lastUsedAt` (S, the
//!   journal's sort-key format). Lowercasing the key keeps `Hades` and `hades` one project.
//! - `sk` = `META#backfill`: present once the registry has been seeded from existing
//!   journal entries, todos and notes.

use std::collections::HashMap;

use aws_sdk_dynamodb::{Client, error::DisplayErrorContext, types::AttributeValue};
use chrono::Utc;

use crate::{
    error::ApiError,
    models::Project,
    repo::{journal, notes, todos},
};

pub const PK_PREFIX: &str = "PROJECT#";
const NAME_PREFIX: &str = "NAME#";
const BACKFILL_SK: &str = "META#backfill";

/// Partition key for a user.
pub fn pk_for(sub: &str) -> String {
    format!("{PK_PREFIX}{sub}")
}

/// Sort key for a project name (already normalized). Case-insensitive.
pub fn sk_for(name: &str) -> String {
    format!("{NAME_PREFIX}{}", name.to_lowercase())
}

/// Most recently used first; ties by name.
pub fn sort_projects(projects: &mut [Project]) {
    projects.sort_by(|a, b| {
        b.last_used_at
            .cmp(&a.last_used_at)
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Fold `(name, usedAt)` pairs into one entry per case-insensitive name, keeping the latest
/// use (and its spelling).
pub fn latest_uses(uses: impl IntoIterator<Item = (String, String)>) -> Vec<Project> {
    let mut by_key: HashMap<String, Project> = HashMap::new();
    for (name, used_at) in uses {
        let key = name.to_lowercase();
        match by_key.get_mut(&key) {
            Some(p) if p.last_used_at >= used_at => {}
            Some(p) => {
                p.name = name;
                p.last_used_at = used_at;
            }
            None => {
                by_key.insert(
                    key,
                    Project {
                        name,
                        last_used_at: used_at,
                    },
                );
            }
        }
    }
    let mut out: Vec<Project> = by_key.into_values().collect();
    sort_projects(&mut out);
    out
}

fn ddb_error<E>(op: &str, e: &aws_sdk_dynamodb::error::SdkError<E>) -> ApiError
where
    E: std::error::Error + Send + Sync + 'static,
{
    ApiError::internal(format!("{op} failed: {}", DisplayErrorContext(e)))
}

pub struct ProjectRepo<'a> {
    client: &'a Client,
    table: &'a str,
}

impl<'a> ProjectRepo<'a> {
    pub fn new(client: &'a Client, table: &'a str) -> Self {
        Self { client, table }
    }

    fn key(sub: &str, sk: &str) -> HashMap<String, AttributeValue> {
        HashMap::from([
            ("pk".to_owned(), AttributeValue::S(pk_for(sub))),
            ("sk".to_owned(), AttributeValue::S(sk.to_owned())),
        ])
    }

    /// Record that `name` was just used. With `keep_newer`, an existing later `lastUsedAt`
    /// (and its spelling) wins; that's for the backfill, which replays old uses.
    async fn put_use(
        &self,
        sub: &str,
        name: &str,
        used_at: &str,
        keep_newer: bool,
    ) -> Result<(), ApiError> {
        let mut req = self
            .client
            .update_item()
            .table_name(self.table)
            .set_key(Some(Self::key(sub, &sk_for(name))))
            .update_expression("SET #name = :name, #lastUsedAt = :at")
            .expression_attribute_names("#name", "name")
            .expression_attribute_names("#lastUsedAt", "lastUsedAt")
            .expression_attribute_values(":name", AttributeValue::S(name.to_owned()))
            .expression_attribute_values(":at", AttributeValue::S(used_at.to_owned()));
        if keep_newer {
            req =
                req.condition_expression("attribute_not_exists(#lastUsedAt) OR #lastUsedAt < :at");
        }
        match req.send().await {
            Ok(_) => Ok(()),
            Err(e)
                if keep_newer
                    && e.as_service_error()
                        .is_some_and(|se| se.is_conditional_check_failed_exception()) =>
            {
                Ok(())
            }
            Err(e) => Err(ddb_error("UpdateItem (project)", &e)),
        }
    }

    /// Record a use of `name` now.
    pub async fn touch(&self, sub: &str, name: &str) -> Result<(), ApiError> {
        self.put_use(sub, name, &journal::format_sk(Utc::now()), false)
            .await
    }

    /// Best-effort `touch` after a successful write elsewhere: the write already happened, so
    /// a registry failure is logged rather than failing the request.
    pub async fn touch_quietly(&self, sub: &str, name: Option<&str>) {
        let Some(name) = name else { return };
        if let Err(e) = self.touch(sub, name).await {
            tracing::warn!(error = ?e, "failed to record project use");
        }
    }

    /// Remove `name` from the registry. `false` if it wasn't there. Entries that use it are
    /// untouched, and using it again adds it back.
    pub async fn delete(&self, sub: &str, name: &str) -> Result<bool, ApiError> {
        let res = self
            .client
            .delete_item()
            .table_name(self.table)
            .set_key(Some(Self::key(sub, &sk_for(name))))
            .condition_expression("attribute_exists(sk)")
            .send()
            .await;
        match res {
            Ok(_) => Ok(true),
            Err(e)
                if e.as_service_error()
                    .is_some_and(|se| se.is_conditional_check_failed_exception()) =>
            {
                Ok(false)
            }
            Err(e) => Err(ddb_error("DeleteItem (project)", &e)),
        }
    }

    /// Every item in a partition, projected to `sk` plus `attrs`.
    async fn query_all(
        &self,
        pk: String,
        attrs: &[&str],
    ) -> Result<Vec<HashMap<String, AttributeValue>>, ApiError> {
        let mut names = HashMap::from([("#sk".to_owned(), "sk".to_owned())]);
        for a in attrs {
            names.insert(format!("#{a}"), (*a).to_owned());
        }
        let projection = std::iter::once("#sk".to_owned())
            .chain(attrs.iter().map(|a| format!("#{a}")))
            .collect::<Vec<_>>()
            .join(", ");
        let mut items = Vec::new();
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;
        loop {
            let out = self
                .client
                .query()
                .table_name(self.table)
                .key_condition_expression("pk = :pk")
                .expression_attribute_values(":pk", AttributeValue::S(pk.clone()))
                .projection_expression(&projection)
                .set_expression_attribute_names(Some(names.clone()))
                .set_exclusive_start_key(start_key.take())
                .send()
                .await
                .map_err(|e| ddb_error("Query (projects)", &e))?;
            items.extend(out.items().iter().cloned());
            match out.last_evaluated_key() {
                Some(k) if !k.is_empty() => start_key = Some(k.clone()),
                _ => break,
            }
        }
        Ok(items)
    }

    /// Seed the registry from every project already used in journal entries, todos and
    /// notes. Idempotent: replaying it never moves a project's `lastUsedAt` backwards.
    async fn backfill(&self, sub: &str) -> Result<(), ApiError> {
        let s = |item: &HashMap<String, AttributeValue>, name: &str| {
            item.get(name).and_then(|v| v.as_s().ok()).cloned()
        };
        let mut uses = Vec::new();
        // A journal entry's sort key is its timestamp.
        for item in self.query_all(journal::pk_for(sub), &["project"]).await? {
            if let (Some(p), Some(at)) = (s(&item, "project"), s(&item, "sk")) {
                uses.push((p, at));
            }
        }
        for (pk, at_attr) in [
            (todos::pk_for(sub), "updatedAt"),
            (notes::pk_for(sub), "updatedAt"),
        ] {
            for item in self.query_all(pk, &["project", at_attr]).await? {
                if let (Some(p), Some(at)) = (s(&item, "project"), s(&item, at_attr)) {
                    uses.push((p, at));
                }
            }
        }
        for p in latest_uses(uses) {
            self.put_use(sub, &p.name, &p.last_used_at, true).await?;
        }
        self.client
            .put_item()
            .table_name(self.table)
            .set_item(Some(Self::key(sub, BACKFILL_SK)))
            .send()
            .await
            .map_err(|e| ddb_error("PutItem (project backfill marker)", &e))?;
        Ok(())
    }

    /// Every registered project, most recently used first. The first call for a user seeds
    /// the registry from their existing data.
    pub async fn list(&self, sub: &str) -> Result<Vec<Project>, ApiError> {
        let attrs = ["name", "lastUsedAt"];
        let mut items = self.query_all(pk_for(sub), &attrs).await?;
        let backfilled = items.iter().any(|i| {
            i.get("sk").and_then(|v| v.as_s().ok()).map(String::as_str) == Some(BACKFILL_SK)
        });
        if !backfilled {
            self.backfill(sub).await?;
            items = self.query_all(pk_for(sub), &attrs).await?;
        }
        let mut projects: Vec<Project> = items.iter().filter_map(project_from_item).collect();
        sort_projects(&mut projects);
        Ok(projects)
    }
}

fn project_from_item(item: &HashMap<String, AttributeValue>) -> Option<Project> {
    let s = |name: &str| item.get(name).and_then(|v| v.as_s().ok()).cloned();
    if !s("sk")?.starts_with(NAME_PREFIX) {
        return None;
    }
    Some(Project {
        name: s("name")?,
        last_used_at: s("lastUsedAt")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, at: &str) -> Project {
        Project {
            name: name.into(),
            last_used_at: at.into(),
        }
    }

    #[test]
    fn keys() {
        assert_eq!(pk_for("abc"), "PROJECT#abc");
        assert_eq!(sk_for("Hades Infra"), "NAME#hades infra");
    }

    #[test]
    fn latest_use_wins_case_insensitively() {
        let got = latest_uses([
            ("hades".into(), "2026-10-01T00:00:00.000Z".into()),
            ("Home".into(), "2026-10-05T00:00:00.000Z".into()),
            ("Hades".into(), "2026-10-09T00:00:00.000Z".into()),
            ("HADES".into(), "2026-10-02T00:00:00.000Z".into()),
        ]);
        assert_eq!(
            got,
            [
                p("Hades", "2026-10-09T00:00:00.000Z"),
                p("Home", "2026-10-05T00:00:00.000Z")
            ]
        );
    }

    #[test]
    fn only_name_items_are_projects() {
        let item = |sk: &str| {
            HashMap::from([
                ("sk".to_owned(), AttributeValue::S(sk.to_owned())),
                ("name".to_owned(), AttributeValue::S("x".to_owned())),
                ("lastUsedAt".to_owned(), AttributeValue::S("t".to_owned())),
            ])
        };
        assert_eq!(project_from_item(&item("NAME#x")), Some(p("x", "t")));
        assert_eq!(project_from_item(&item(BACKFILL_SK)), None);
    }
}
