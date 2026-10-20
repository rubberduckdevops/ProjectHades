//! Search logic (see "`GET /api/search`" in API_CONTRACT.md): the vector filter, name matching,
//! and merging/ranking of name and semantic matches. Pure functions; the handler does the I/O.

use std::collections::HashMap;

use aws_smithy_types::Document;
use serde::Serialize;

use crate::{
    models::NoteMeta,
    vectors::{HitMetadata, VectorMatch, path_from_key},
};

/// `topK` for `QueryVectors`.
pub const TOP_K: i32 = 30;
pub const MAX_QUERY_CHARS: usize = 500;
pub const DEFAULT_LIMIT: usize = 10;
pub const MAX_LIMIT: usize = 50;

/// The `QueryVectors` filter. **This is the only place the `sub` condition is built**; it
/// isolates users from each other, so `sub` must always come from the verified token.
pub fn search_filter(sub: &str, project: Option<&str>) -> Document {
    let eq = |field: &str, value: &str| {
        Document::Object(HashMap::from([(
            field.to_owned(),
            Document::Object(HashMap::from([("$eq".to_owned(), Document::from(value))])),
        )]))
    };
    let sub_filter = eq("sub", sub);
    match project {
        None => sub_filter,
        Some(p) => Document::Object(HashMap::from([(
            "$and".to_owned(),
            Document::Array(vec![sub_filter, eq("project", p)]),
        )])),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchKind {
    Name,
    Semantic,
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub path: String,
    pub title: String,
    pub project: Option<String>,
    pub updated_at: String,
    #[serde(rename = "match")]
    pub match_kind: MatchKind,
    pub score: Option<f32>,
    pub heading: Option<String>,
    pub snippet: Option<String>,
}

/// A note's best semantic chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticHit {
    pub path: String,
    pub score: f32,
    pub heading: Option<String>,
    pub snippet: Option<String>,
}

/// Group vector matches by note path, keeping each note's best chunk (`score = 1 - distance`),
/// sorted by score descending. Matches that don't belong to `sub` (by key or metadata) are
/// dropped as a second line of defense behind the filter.
pub fn best_hits(sub: &str, matches: &[VectorMatch]) -> Vec<SemanticHit> {
    let mut best: HashMap<String, SemanticHit> = HashMap::new();
    for m in matches {
        let Some(key_path) = path_from_key(sub, &m.key) else {
            continue;
        };
        let meta = m
            .metadata
            .as_ref()
            .map(HitMetadata::from_document)
            .unwrap_or_default();
        if meta.sub.as_deref().is_some_and(|s| s != sub) {
            continue;
        }
        let path = meta.path.unwrap_or_else(|| key_path.to_owned());
        let score = 1.0 - m.distance.unwrap_or(1.0);
        let hit = SemanticHit {
            path: path.clone(),
            score,
            heading: meta.heading,
            snippet: meta.snippet,
        };
        match best.get(&path) {
            Some(b) if b.score >= score => {}
            _ => {
                best.insert(path, hit);
            }
        }
    }
    let mut hits: Vec<SemanticHit> = best.into_values().collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.cmp(&b.path))
    });
    hits
}

fn project_matches(note: &NoteMeta, project: Option<&str>) -> bool {
    project.is_none_or(|p| note.project.as_deref() == Some(p))
}

/// Notes whose title or path contains `q` (case-insensitive) and that pass the project filter:
/// title matches first, then path-only matches, each by `updatedAt` descending.
pub fn name_matches<'a>(
    q: &str,
    project: Option<&str>,
    notes: &'a [NoteMeta],
) -> Vec<&'a NoteMeta> {
    let q = q.to_lowercase();
    let mut matches: Vec<(bool, &NoteMeta)> = notes
        .iter()
        .filter(|n| project_matches(n, project))
        .filter_map(|n| {
            let in_title = n.title.to_lowercase().contains(&q);
            (in_title || n.path.to_lowercase().contains(&q)).then_some((in_title, n))
        })
        .collect();
    matches.sort_by(|(ta, a), (tb, b)| {
        tb.cmp(ta)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.path.cmp(&b.path))
    });
    matches.into_iter().map(|(_, n)| n).collect()
}

/// Merge name matches (first) and semantic hits, deduplicated by path, joined with the
/// caller's current note metadata (semantic hits without metadata are dropped), trimmed to
/// `limit`.
pub fn merge_results(
    q: &str,
    project: Option<&str>,
    notes: &[NoteMeta],
    hits: Vec<SemanticHit>,
    limit: usize,
) -> Vec<SearchResult> {
    let by_path: HashMap<&str, &NoteMeta> = notes.iter().map(|n| (n.path.as_str(), n)).collect();
    let mut semantic: HashMap<String, SemanticHit> = HashMap::new();
    let mut semantic_order = Vec::new();
    for h in hits {
        if !semantic.contains_key(&h.path) {
            semantic_order.push(h.path.clone());
            semantic.insert(h.path.clone(), h);
        }
    }

    let result = |n: &NoteMeta, kind: MatchKind, hit: Option<SemanticHit>| SearchResult {
        path: n.path.clone(),
        title: n.title.clone(),
        project: n.project.clone(),
        updated_at: n.updated_at.clone(),
        match_kind: kind,
        score: hit.as_ref().map(|h| h.score),
        heading: hit.as_ref().and_then(|h| h.heading.clone()),
        snippet: hit.and_then(|h| h.snippet),
    };

    let mut out: Vec<SearchResult> = name_matches(q, project, notes)
        .into_iter()
        .map(|n| result(n, MatchKind::Name, semantic.remove(&n.path)))
        .collect();
    for path in semantic_order {
        let Some(hit) = semantic.remove(&path) else {
            continue; // already shown as a name match
        };
        let Some(note) = by_path.get(path.as_str()) else {
            continue; // deleted or in flight
        };
        if !project_matches(note, project) {
            continue; // project changed since it was indexed
        }
        out.push(result(note, MatchKind::Semantic, Some(hit)));
    }
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectors::{ChunkMetadata, doc_to_json, vector_key};
    use serde_json::json;

    #[test]
    fn filter_without_project() {
        assert_eq!(
            doc_to_json(&search_filter("user-1", None)),
            json!({ "sub": { "$eq": "user-1" } })
        );
    }

    #[test]
    fn filter_with_project_keeps_sub() {
        assert_eq!(
            doc_to_json(&search_filter("user-1", Some("hades"))),
            json!({ "$and": [
                { "sub": { "$eq": "user-1" } },
                { "project": { "$eq": "hades" } }
            ] })
        );
    }

    #[test]
    fn filter_values_are_not_interpreted() {
        // Operator-looking input stays a plain string value.
        assert_eq!(
            doc_to_json(&search_filter("u", Some("{\"$ne\": 1}"))),
            json!({ "$and": [
                { "sub": { "$eq": "u" } },
                { "project": { "$eq": "{\"$ne\": 1}" } }
            ] })
        );
    }

    fn note(path: &str, title: &str, project: Option<&str>, updated: &str) -> NoteMeta {
        NoteMeta {
            path: path.into(),
            title: title.into(),
            project: project.map(str::to_owned),
            tags: vec![],
            content_hash: "h".into(),
            size: 1,
            created_at: updated.into(),
            updated_at: updated.into(),
            indexed: true,
        }
    }

    fn vm(sub: &str, path: &str, n: u32, distance: f32, heading: &str) -> VectorMatch {
        VectorMatch {
            key: vector_key(sub, path, n),
            distance: Some(distance),
            metadata: Some(
                ChunkMetadata {
                    sub: sub.into(),
                    path: path.into(),
                    project: None,
                    heading: heading.into(),
                    snippet: format!("snippet {n}"),
                    title: "t".into(),
                    updated_at: "u".into(),
                }
                .to_document(),
            ),
        }
    }

    #[test]
    fn best_hits_groups_by_path() {
        let hits = best_hits(
            "u",
            &[
                vm("u", "a.md", 0, 0.5, "A0"),
                vm("u", "a.md", 1, 0.2, "A1"),
                vm("u", "b.md", 0, 0.1, "B0"),
                vm("u", "a.md", 2, 0.9, "A2"),
            ],
        );
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "b.md");
        assert!((hits[0].score - 0.9).abs() < 1e-6);
        assert_eq!(hits[1].path, "a.md");
        assert_eq!(hits[1].heading.as_deref(), Some("A1"));
        assert_eq!(hits[1].snippet.as_deref(), Some("snippet 1"));
    }

    #[test]
    fn best_hits_drops_foreign_results() {
        let mut spoofed = vm("u", "c.md", 0, 0.0, "C");
        spoofed.metadata = Some(
            ChunkMetadata {
                sub: "other".into(),
                path: "c.md".into(),
                project: None,
                heading: "x".into(),
                snippet: "x".into(),
                title: "x".into(),
                updated_at: "x".into(),
            }
            .to_document(),
        );
        let hits = best_hits(
            "u",
            &[
                vm("other", "a.md", 0, 0.0, "X"),
                spoofed,
                vm("u", "b.md", 0, 0.3, "B"),
            ],
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "b.md");
    }

    #[test]
    fn best_hits_without_metadata_uses_key_path() {
        let m = VectorMatch {
            key: "u#x/#y.md#3".into(),
            distance: Some(0.25),
            metadata: None,
        };
        let hits = best_hits("u", &[m]);
        assert_eq!(hits[0].path, "x/#y.md");
        assert_eq!(hits[0].heading, None);
    }

    #[test]
    fn name_match_ranking() {
        let notes = vec![
            note("Projects/Design.md", "Architecture", None, "2026-10-03"),
            note("Inbox/design-old.md", "Old Design", None, "2026-10-01"),
            note("Inbox/b.md", "Design notes", Some("hades"), "2026-10-02"),
            note("Inbox/c.md", "Unrelated", None, "2026-10-09"),
        ];
        let paths: Vec<&str> = name_matches("DESIGN", None, &notes)
            .iter()
            .map(|n| n.path.as_str())
            .collect();
        // Title matches by updatedAt desc, then path-only matches.
        assert_eq!(
            paths,
            ["Inbox/b.md", "Inbox/design-old.md", "Projects/Design.md"]
        );
        let only_hades: Vec<&str> = name_matches("design", Some("hades"), &notes)
            .iter()
            .map(|n| n.path.as_str())
            .collect();
        assert_eq!(only_hades, ["Inbox/b.md"]);
    }

    fn hit(path: &str, score: f32) -> SemanticHit {
        SemanticHit {
            path: path.into(),
            score,
            heading: Some(format!("{path} heading")),
            snippet: Some(format!("{path} snippet")),
        }
    }

    #[test]
    fn merge_puts_name_matches_first_and_dedupes() {
        let notes = vec![
            note("storage.md", "Storage", None, "2026-10-01"),
            note("a.md", "Alpha", None, "2026-10-01"),
            note("b.md", "Beta", None, "2026-10-01"),
        ];
        let hits = vec![hit("a.md", 0.9), hit("storage.md", 0.8), hit("b.md", 0.7)];
        let r = merge_results("storage", None, &notes, hits, 10);
        let summary: Vec<(&str, MatchKind)> =
            r.iter().map(|r| (r.path.as_str(), r.match_kind)).collect();
        assert_eq!(
            summary,
            [
                ("storage.md", MatchKind::Name),
                ("a.md", MatchKind::Semantic),
                ("b.md", MatchKind::Semantic)
            ]
        );
        // The name match carries its semantic snippet.
        assert_eq!(r[0].snippet.as_deref(), Some("storage.md snippet"));
        assert_eq!(r[0].score, Some(0.8));
    }

    #[test]
    fn merge_name_match_without_semantic_hit_has_nulls() {
        let notes = vec![note("x/Storage.md", "S", Some("p"), "2026-10-01")];
        let r = merge_results("storage", None, &notes, vec![], 10);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(
            v,
            json!([{
                "path": "x/Storage.md", "title": "S", "project": "p", "updatedAt": "2026-10-01",
                "match": "name", "score": null, "heading": null, "snippet": null
            }])
        );
    }

    #[test]
    fn merge_drops_hits_without_metadata_and_wrong_project() {
        let notes = vec![
            note("a.md", "A", Some("p"), "1"),
            note("b.md", "B", Some("q"), "1"),
        ];
        let hits = vec![hit("gone.md", 0.99), hit("b.md", 0.9), hit("a.md", 0.5)];
        let r = merge_results("zzz", Some("p"), &notes, hits.clone(), 10);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].path, "a.md");
        assert_eq!(r[0].match_kind, MatchKind::Semantic);
        let r = merge_results("zzz", None, &notes, hits, 10);
        let paths: Vec<&str> = r.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["b.md", "a.md"]);
    }

    #[test]
    fn merge_trims_to_limit() {
        let notes: Vec<NoteMeta> = (0..5)
            .map(|i| note(&format!("n{i}.md"), "Note", None, &format!("{i}")))
            .collect();
        let hits = (0..5).map(|i| hit(&format!("n{i}.md"), 0.5)).collect();
        let r = merge_results("note", None, &notes, hits, 3);
        let paths: Vec<&str> = r.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["n4.md", "n3.md", "n2.md"]);
    }
}
