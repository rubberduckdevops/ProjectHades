//! DynamoDB access for todos.
//!
//! Item layout: `pk` (S) = `TODO#<sub>`, `sk` (S) = creation time in the journal's sort-key
//! format (also the todo's id), `title` (S), `done` (BOOL), `updatedAt` (S), and optional
//! `project` (S), `due` (S, `YYYY-MM-DD`) and `completedAt` (S).

use std::collections::HashMap;

use aws_sdk_dynamodb::{
    Client,
    error::DisplayErrorContext,
    types::{AttributeValue, ReturnValue},
};
use chrono::Utc;

use crate::{error::ApiError, models::Todo, repo::journal::format_sk};

pub const PK_PREFIX: &str = "TODO#";

/// Partition key for a user.
pub fn pk_for(sub: &str) -> String {
    format!("{PK_PREFIX}{sub}")
}

/// An already-validated partial update. See `PatchTodo` for the `Option<Option<_>>` meaning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoUpdate {
    pub title: Option<String>,
    pub project: Option<Option<String>>,
    pub due: Option<Option<String>>,
    pub done: Option<bool>,
}

/// The `UpdateItem` expression for `update` at time `now`: `(expression, names, values)`.
/// Every attribute goes through a `#name` placeholder, so reserved words can't bite.
pub fn update_expression(
    update: &TodoUpdate,
    now: &str,
) -> (
    String,
    HashMap<String, String>,
    HashMap<String, AttributeValue>,
) {
    let mut set = vec!["#updatedAt = :now".to_owned()];
    let mut remove = Vec::new();
    let mut names = HashMap::from([("#updatedAt".to_owned(), "updatedAt".to_owned())]);
    let mut values = HashMap::from([(":now".to_owned(), AttributeValue::S(now.to_owned()))]);

    if let Some(title) = &update.title {
        set.push("#title = :title".to_owned());
        names.insert("#title".to_owned(), "title".to_owned());
        values.insert(":title".to_owned(), AttributeValue::S(title.clone()));
    }
    for (attr, field) in [("project", &update.project), ("due", &update.due)] {
        match field {
            None => {}
            Some(Some(v)) => {
                set.push(format!("#{attr} = :{attr}"));
                names.insert(format!("#{attr}"), attr.to_owned());
                values.insert(format!(":{attr}"), AttributeValue::S(v.clone()));
            }
            Some(None) => {
                remove.push(format!("#{attr}"));
                names.insert(format!("#{attr}"), attr.to_owned());
            }
        }
    }
    if let Some(done) = update.done {
        set.push("#done = :done".to_owned());
        names.insert("#done".to_owned(), "done".to_owned());
        values.insert(":done".to_owned(), AttributeValue::Bool(done));
        names.insert("#completedAt".to_owned(), "completedAt".to_owned());
        if done {
            // Re-completing a done todo keeps its original completion time.
            set.push("#completedAt = if_not_exists(#completedAt, :now)".to_owned());
        } else {
            remove.push("#completedAt".to_owned());
        }
    }

    let mut expr = format!("SET {}", set.join(", "));
    if !remove.is_empty() {
        expr.push_str(&format!(" REMOVE {}", remove.join(", ")));
    }
    (expr, names, values)
}

pub struct TodoRepo<'a> {
    client: &'a Client,
    table: &'a str,
}

fn ddb_error<E>(op: &str, e: &aws_sdk_dynamodb::error::SdkError<E>) -> ApiError
where
    E: std::error::Error + Send + Sync + 'static,
{
    ApiError::internal(format!("{op} failed: {}", DisplayErrorContext(e)))
}

impl<'a> TodoRepo<'a> {
    pub fn new(client: &'a Client, table: &'a str) -> Self {
        Self { client, table }
    }

    fn key(sub: &str, id: &str) -> HashMap<String, AttributeValue> {
        HashMap::from([
            ("pk".to_owned(), AttributeValue::S(pk_for(sub))),
            ("sk".to_owned(), AttributeValue::S(id.to_owned())),
        ])
    }

    fn new_item(sub: &str, todo: &Todo) -> HashMap<String, AttributeValue> {
        let mut item = Self::key(sub, &todo.id);
        item.insert("title".to_owned(), AttributeValue::S(todo.title.clone()));
        item.insert("done".to_owned(), AttributeValue::Bool(todo.done));
        item.insert(
            "updatedAt".to_owned(),
            AttributeValue::S(todo.updated_at.clone()),
        );
        if let Some(p) = &todo.project {
            item.insert("project".to_owned(), AttributeValue::S(p.clone()));
        }
        if let Some(d) = &todo.due {
            item.insert("due".to_owned(), AttributeValue::S(d.clone()));
        }
        item
    }

    /// Create an open todo at `now`. Never overwrites: on a sort-key collision it retries
    /// once with a fresh timestamp.
    pub async fn create(
        &self,
        sub: &str,
        title: &str,
        project: Option<&str>,
        due: Option<&str>,
    ) -> Result<Todo, ApiError> {
        let mut attempt = 0;
        loop {
            let now = format_sk(Utc::now());
            let todo = Todo {
                id: now.clone(),
                title: title.to_owned(),
                project: project.map(str::to_owned),
                due: due.map(str::to_owned),
                done: false,
                created_at: now.clone(),
                completed_at: None,
                updated_at: now,
            };
            let res = self
                .client
                .put_item()
                .table_name(self.table)
                .set_item(Some(Self::new_item(sub, &todo)))
                .condition_expression("attribute_not_exists(sk)")
                .send()
                .await;
            match res {
                Ok(_) => return Ok(todo),
                Err(e)
                    if attempt == 0
                        && e.as_service_error()
                            .is_some_and(|se| se.is_conditional_check_failed_exception()) =>
                {
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
                Err(e) => return Err(ddb_error("PutItem (todo)", &e)),
            }
        }
    }

    /// Every todo for the user, oldest first.
    pub async fn list(&self, sub: &str) -> Result<Vec<Todo>, ApiError> {
        let mut todos = Vec::new();
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;
        loop {
            let out = self
                .client
                .query()
                .table_name(self.table)
                .key_condition_expression("pk = :pk")
                .expression_attribute_values(":pk", AttributeValue::S(pk_for(sub)))
                .scan_index_forward(true)
                .set_exclusive_start_key(start_key.take())
                .send()
                .await
                .map_err(|e| ddb_error("Query (todos)", &e))?;
            for item in out.items() {
                todos.push(todo_from_item(item)?);
            }
            match out.last_evaluated_key() {
                Some(k) if !k.is_empty() => start_key = Some(k.clone()),
                _ => break,
            }
        }
        Ok(todos)
    }

    /// Apply `update` to an existing todo. `Ok(None)` if it doesn't exist.
    pub async fn update(
        &self,
        sub: &str,
        id: &str,
        update: &TodoUpdate,
    ) -> Result<Option<Todo>, ApiError> {
        let (expr, names, values) = update_expression(update, &format_sk(Utc::now()));
        let res = self
            .client
            .update_item()
            .table_name(self.table)
            .set_key(Some(Self::key(sub, id)))
            .update_expression(expr)
            .condition_expression("attribute_exists(sk)")
            .set_expression_attribute_names(Some(names))
            .set_expression_attribute_values(Some(values))
            .return_values(ReturnValue::AllNew)
            .send()
            .await;
        match res {
            Ok(out) => out.attributes().map(todo_from_item).transpose(),
            Err(e)
                if e.as_service_error()
                    .is_some_and(|se| se.is_conditional_check_failed_exception()) =>
            {
                Ok(None)
            }
            Err(e) => Err(ddb_error("UpdateItem (todo)", &e)),
        }
    }

    /// Delete a todo. Returns `false` if it didn't exist.
    pub async fn delete(&self, sub: &str, id: &str) -> Result<bool, ApiError> {
        let res = self
            .client
            .delete_item()
            .table_name(self.table)
            .set_key(Some(Self::key(sub, id)))
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
            Err(e) => Err(ddb_error("DeleteItem (todo)", &e)),
        }
    }
}

fn todo_from_item(item: &HashMap<String, AttributeValue>) -> Result<Todo, ApiError> {
    let s = |name: &str| {
        item.get(name)
            .and_then(|v| v.as_s().ok())
            .cloned()
            .ok_or_else(|| ApiError::internal(format!("todo missing string attribute {name}")))
    };
    let opt = |name: &str| item.get(name).and_then(|v| v.as_s().ok()).cloned();
    let sk = s("sk")?;
    Ok(Todo {
        id: sk.clone(),
        title: s("title")?,
        project: opt("project"),
        due: opt("due"),
        done: item
            .get("done")
            .and_then(|v| v.as_bool().ok())
            .copied()
            .unwrap_or(false),
        created_at: sk,
        completed_at: opt("completedAt"),
        updated_at: s("updatedAt")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-10-20T14:03:22.123Z";

    fn todo() -> Todo {
        Todo {
            id: NOW.into(),
            title: "Renew passport".into(),
            project: Some("home".into()),
            due: Some("2026-11-01".into()),
            done: false,
            created_at: NOW.into(),
            completed_at: None,
            updated_at: NOW.into(),
        }
    }

    #[test]
    fn pk_builder() {
        assert_eq!(pk_for("abc"), "TODO#abc");
    }

    #[test]
    fn item_round_trips() {
        let t = todo();
        let item = TodoRepo::new_item("u", &t);
        assert_eq!(item["pk"].as_s().unwrap(), "TODO#u");
        assert_eq!(item["sk"].as_s().unwrap(), NOW);
        assert!(!item.contains_key("completedAt"));
        assert_eq!(todo_from_item(&item).unwrap(), t);
    }

    #[test]
    fn item_without_optionals() {
        let t = Todo {
            project: None,
            due: None,
            ..todo()
        };
        let item = TodoRepo::new_item("u", &t);
        assert!(!item.contains_key("project"));
        assert!(!item.contains_key("due"));
        assert_eq!(todo_from_item(&item).unwrap(), t);
    }

    #[test]
    fn expression_only_touches_updated_at_for_empty_patch() {
        let (expr, names, values) = update_expression(&TodoUpdate::default(), NOW);
        assert_eq!(expr, "SET #updatedAt = :now");
        assert_eq!(names.len(), 1);
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn expression_sets_and_clears_fields() {
        let update = TodoUpdate {
            title: Some("New".into()),
            project: Some(None),
            due: Some(Some("2026-12-01".into())),
            done: None,
        };
        let (expr, names, values) = update_expression(&update, NOW);
        assert_eq!(
            expr,
            "SET #updatedAt = :now, #title = :title, #due = :due REMOVE #project"
        );
        assert_eq!(names["#project"], "project");
        assert_eq!(values[":due"].as_s().unwrap(), "2026-12-01");
        assert!(!values.contains_key(":project"));
    }

    #[test]
    fn expression_for_done_keeps_first_completion_time() {
        let done = TodoUpdate {
            done: Some(true),
            ..Default::default()
        };
        let (expr, _, values) = update_expression(&done, NOW);
        assert_eq!(
            expr,
            "SET #updatedAt = :now, #done = :done, #completedAt = if_not_exists(#completedAt, :now)"
        );
        assert_eq!(values[":done"], AttributeValue::Bool(true));

        let undone = TodoUpdate {
            done: Some(false),
            ..Default::default()
        };
        let (expr, _, _) = update_expression(&undone, NOW);
        assert_eq!(
            expr,
            "SET #updatedAt = :now, #done = :done REMOVE #completedAt"
        );
    }
}
