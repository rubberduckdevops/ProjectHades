//! Pure helpers for notes: path validation, hashing, S3 keys and metadata derivation from
//! Markdown content (see "Notes (phase 1)" in API_CONTRACT.md).

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_norway::Value;
use sha2::{Digest, Sha256};

/// Maximum note path length in bytes.
pub const MAX_PATH_BYTES: usize = 512;
/// Maximum note content size in bytes (1 MB).
pub const MAX_CONTENT_BYTES: usize = 1024 * 1024;

/// Derived metadata is capped so a pathological frontmatter can never push the DynamoDB item
/// past its 400 KB limit. Values are truncated (on a char boundary), never rejected.
pub const MAX_TITLE_CHARS: usize = 500;
pub const MAX_PROJECT_CHARS: usize = 200;
pub const MAX_TAGS: usize = 100;
pub const MAX_TAG_CHARS: usize = 100;

/// Validate a note path (the note's ID). Returns a human-readable reason on failure.
pub fn validate_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Err("path must not be empty");
    }
    if path.len() > MAX_PATH_BYTES {
        return Err("path must be at most 512 bytes");
    }
    if !path.ends_with(".md") {
        return Err("path must end in .md");
    }
    if path.contains('\\') {
        return Err("path must not contain a backslash");
    }
    if path.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
        return Err("path must not contain control characters");
    }
    if path.starts_with('/') || path.ends_with('/') {
        return Err("path must not start or end with /");
    }
    for segment in path.split('/') {
        match segment {
            "" => return Err("path must not contain empty segments"),
            "." | ".." => return Err("path must not contain . or .. segments"),
            _ => {}
        }
        if segment.starts_with(char::is_whitespace) || segment.ends_with(char::is_whitespace) {
            return Err("path segments must not start or end with whitespace");
        }
    }
    Ok(())
}

/// Whether `s` looks like a sha256 hex digest (64 hex digits, any case).
pub fn is_valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Lowercase hex sha256 of the UTF-8 content.
pub fn sha256_hex(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

/// S3 object key for a note: `users/<sub>/notes/<path>`.
pub fn s3_key(sub: &str, path: &str) -> String {
    format!("users/{sub}/notes/{path}")
}

/// Characters left as-is in a `CopySource` key: RFC 3986 unreserved characters plus `/`.
const COPY_SOURCE_KEEP: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

/// `CopySource` value for S3 `CopyObject`: `<bucket>/<url-encoded key>?versionId=<id>`.
pub fn copy_source(bucket: &str, key: &str, version_id: &str) -> String {
    format!(
        "{bucket}/{}?versionId={}",
        utf8_percent_encode(key, COPY_SOURCE_KEEP),
        utf8_percent_encode(version_id, COPY_SOURCE_KEEP)
    )
}

/// Metadata derived from a note's path and content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedMeta {
    pub title: String,
    pub project: Option<String>,
    pub tags: Vec<String>,
}

/// Split leading YAML frontmatter (between `---` lines) from the body.
/// Returns `None` if the content does not start with a closed frontmatter block.
pub fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let first_end = content.find('\n')?;
    if content[..first_end].trim_end() != "---" {
        return None;
    }
    let yaml_start = first_end + 1;
    let mut pos = yaml_start;
    while pos <= content.len() {
        let line_end = content[pos..].find('\n').map_or(content.len(), |i| pos + i);
        if content[pos..line_end].trim_end() == "---" {
            let body_start = (line_end + 1).min(content.len());
            return Some((&content[yaml_start..pos], &content[body_start..]));
        }
        if line_end == content.len() {
            break;
        }
        pos = line_end + 1;
    }
    None
}

/// The content without its frontmatter. Malformed frontmatter is kept (it counts as body, as
/// in `derive_metadata`).
pub fn strip_frontmatter(content: &str) -> &str {
    match split_frontmatter(content) {
        Some((yaml, body)) if parse_frontmatter(yaml).is_some() => body,
        _ => content,
    }
}

/// Parse frontmatter YAML. Anything that isn't a mapping (or empty) counts as malformed.
fn parse_frontmatter(yaml: &str) -> Option<serde_norway::Mapping> {
    match serde_norway::from_str::<Value>(yaml).ok()? {
        Value::Mapping(m) => Some(m),
        Value::Null => Some(serde_norway::Mapping::new()),
        _ => None,
    }
}

/// A scalar frontmatter value as a string (strings and numbers; not bools, lists or maps).
fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Tagged(t) => scalar_string(&t.value),
        _ => None,
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_owned(),
        None => s.to_owned(),
    }
}

/// Trimmed, non-blank, truncated.
fn clean(s: &str, max: usize) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| truncate_chars(t, max))
}

fn tags_from(v: &Value) -> Vec<String> {
    let raw: Vec<String> = match v {
        Value::Sequence(items) => items.iter().filter_map(scalar_string).collect(),
        Value::Tagged(t) => return tags_from(&t.value),
        other => scalar_string(other)
            .map(|s| s.split(',').map(str::to_owned).collect())
            .unwrap_or_default(),
    };
    raw.iter()
        .filter_map(|t| {
            let t = t.trim();
            let t = t.strip_prefix('#').unwrap_or(t).trim();
            clean(t, MAX_TAG_CHARS)
        })
        .take(MAX_TAGS)
        .collect()
}

/// Is this line a fenced-code-block delimiter (``` or ~~~, up to 3 spaces of indent)?
pub fn fence_marker(line: &str) -> Option<char> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let t = &line[indent..];
    ['`', '~']
        .into_iter()
        .find(|&c| t.starts_with(&String::from(c).repeat(3)))
}

/// The text of the first `# ` H1 line, outside fenced code blocks.
pub fn first_h1(body: &str) -> Option<String> {
    let mut fence: Option<char> = None;
    for line in body.lines() {
        if let Some(c) = fence_marker(line) {
            match fence {
                None => fence = Some(c),
                Some(open) if open == c => fence = None,
                Some(_) => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            // Drop an optional closing sequence of `#`s ("# Title ##").
            let rest = rest.trim();
            let stripped = rest.trim_end_matches('#');
            let text = if stripped.len() < rest.len()
                && (stripped.is_empty() || stripped.ends_with(char::is_whitespace))
            {
                stripped
            } else {
                rest
            };
            if let Some(t) = clean(text, MAX_TITLE_CHARS) {
                return Some(t);
            }
        }
    }
    None
}

/// Filename without the `.md` extension (falls back to the whole filename if that is empty).
pub fn filename_title(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.strip_suffix(".md").unwrap_or(name);
    truncate_chars(if stem.is_empty() { name } else { stem }, MAX_TITLE_CHARS)
}

/// Derive `title`, `project` and `tags` from a note per the contract. Malformed frontmatter
/// is ignored (the whole content is treated as body).
pub fn derive_metadata(path: &str, content: &str) -> DerivedMeta {
    let (front, body) = match split_frontmatter(content) {
        Some((yaml, body)) => match parse_frontmatter(yaml) {
            Some(map) => (Some(map), body),
            None => (None, content),
        },
        None => (None, content),
    };
    let field = |name: &str| front.as_ref().and_then(|m| m.get(name));

    let title = field("title")
        .and_then(scalar_string)
        .and_then(|t| clean(&t, MAX_TITLE_CHARS))
        .or_else(|| first_h1(body))
        .unwrap_or_else(|| filename_title(path));
    let project = field("project")
        .and_then(scalar_string)
        .and_then(|p| clean(&p, MAX_PROJECT_CHARS));
    let tags = field("tags").map(tags_from).unwrap_or_default();

    DerivedMeta {
        title,
        project,
        tags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_paths() {
        for p in [
            "a.md",
            "Design.md",
            "Projects/Hades/Design.md",
            "Projects/My Ideas/Design.md",
            "Ünïcødé/日本語.md",
            "with.dots/file.name.md",
            "...md",
            "%20.md",
            ".hidden/a.md",
            "a b.md",
        ] {
            assert_eq!(validate_path(p), Ok(()), "{p:?}");
        }
    }

    #[test]
    fn invalid_paths() {
        for p in [
            "",
            "noext",
            "a.MD",
            "a.md.txt",
            "a/../b.md",
            "../b.md",
            "a/./b.md",
            "./b.md",
            "a/..",
            "a//b.md",
            "/a.md",
            "a.md/",
            "/",
            "a\\b.md",
            "a\nb.md",
            "a\tb.md",
            "a\u{0}b.md",
            "a\u{1f}b.md",
            "a\u{7f}b.md",
            " a.md",
            "a.md ",
            "a /b.md",
            "a/ b.md",
            "\u{a0}a/b.md",
            "folder /b.md",
        ] {
            assert!(validate_path(p).is_err(), "{p:?} should be invalid");
        }
    }

    #[test]
    fn path_length_limit_is_in_bytes() {
        let ok = format!("{}.md", "a".repeat(MAX_PATH_BYTES - 3));
        assert_eq!(ok.len(), 512);
        assert_eq!(validate_path(&ok), Ok(()));
        let too_long = format!("{}.md", "a".repeat(MAX_PATH_BYTES - 2));
        assert!(validate_path(&too_long).is_err());
        // 3-byte chars: 170 * 3 + 3 = 513 bytes but only 173 chars.
        let multibyte = format!("{}.md", "€".repeat(170));
        assert!(validate_path(&multibyte).is_err());
    }

    #[test]
    fn hashes() {
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(is_valid_hash(&sha256_hex("x")));
        assert!(is_valid_hash(&"A".repeat(64)));
        assert!(!is_valid_hash(&"a".repeat(63)));
        assert!(!is_valid_hash(&"g".repeat(64)));
        assert!(!is_valid_hash(""));
    }

    #[test]
    fn keys() {
        assert_eq!(
            s3_key("sub-1", "Projects/Hades/Design.md"),
            "users/sub-1/notes/Projects/Hades/Design.md"
        );
        assert_eq!(
            copy_source("bkt", "users/s/notes/My Ideas/a+b.md", "v1.x_Y"),
            "bkt/users/s/notes/My%20Ideas/a%2Bb.md?versionId=v1.x_Y"
        );
        assert_eq!(
            copy_source("bkt", "users/s/notes/Ü.md", "v"),
            "bkt/users/s/notes/%C3%9C.md?versionId=v"
        );
    }

    #[test]
    fn splits_frontmatter() {
        assert_eq!(
            split_frontmatter("---\ntitle: x\n---\nbody\n"),
            Some(("title: x\n", "body\n"))
        );
        assert_eq!(
            split_frontmatter("---\r\ntitle: x\r\n---\r\nbody"),
            Some(("title: x\r\n", "body"))
        );
        assert_eq!(split_frontmatter("---\n---\n"), Some(("", "")));
        assert_eq!(split_frontmatter("---\na: 1\n---"), Some(("a: 1\n", "")));
        assert_eq!(
            split_frontmatter("\u{feff}---\na: 1\n---\nb"),
            Some(("a: 1\n", "b"))
        );
        assert_eq!(split_frontmatter("---\nunclosed\n"), None);
        assert_eq!(split_frontmatter("body\n---\na\n---\n"), None);
        assert_eq!(split_frontmatter("--- \na\n---  \nb"), Some(("a\n", "b")));
        assert_eq!(split_frontmatter("----\na\n---\n"), None);
        assert_eq!(split_frontmatter(""), None);
    }

    fn derive(content: &str) -> DerivedMeta {
        derive_metadata("Projects/Hades/Design Doc.md", content)
    }

    #[test]
    fn title_from_frontmatter_then_h1_then_filename() {
        assert_eq!(
            derive("---\ntitle: From FM\n---\n# Heading\n").title,
            "From FM"
        );
        assert_eq!(
            derive("---\ntitle: \"  \"\n---\n# Heading\n").title,
            "Heading"
        );
        assert_eq!(derive("intro\n# Heading  \n# Second\n").title, "Heading");
        assert_eq!(derive("## Not h1\n#NoSpace\n").title, "Design Doc");
        assert_eq!(derive("").title, "Design Doc");
        assert_eq!(derive("---\ntitle: 2024\n---\n").title, "2024");
        assert_eq!(derive("# Closed ##\n").title, "Closed");
        assert_eq!(derive("# C# notes\n").title, "C# notes");
        assert_eq!(derive("#  \n# Real\n").title, "Real");
    }

    #[test]
    fn h1_inside_code_fence_is_ignored() {
        assert_eq!(derive("```bash\n# comment\n```\n# Real\n").title, "Real");
        assert_eq!(derive("~~~\n# a\n```\n# b\n~~~\n").title, "Design Doc");
    }

    #[test]
    fn filename_titles() {
        assert_eq!(filename_title("a/b/Note.md"), "Note");
        assert_eq!(filename_title("Note.md"), "Note");
        assert_eq!(filename_title("x/.md"), ".md");
    }

    #[test]
    fn project_rules() {
        assert_eq!(
            derive("---\nproject: \" hades \"\n---\n")
                .project
                .as_deref(),
            Some("hades")
        );
        assert_eq!(derive("---\nproject: \"  \"\n---\n").project, None);
        assert_eq!(derive("---\nproject:\n---\n").project, None);
        assert_eq!(derive("---\nproject: [a, b]\n---\n").project, None);
        assert_eq!(derive("no frontmatter").project, None);
    }

    #[test]
    fn tag_rules() {
        let t = |c: &str| derive(c).tags;
        assert_eq!(
            t("---\ntags: [a, \" b \", \"#c\", \"\"]\n---\n"),
            ["a", "b", "c"]
        );
        assert_eq!(t("---\ntags:\n  - x\n  - '#y'\n---\n"), ["x", "y"]);
        assert_eq!(t("---\ntags: \"a, #b , ,c\"\n---\n"), ["a", "b", "c"]);
        assert_eq!(t("---\ntags: solo\n---\n"), ["solo"]);
        assert_eq!(t("---\ntags: [2024, true]\n---\n"), ["2024"]);
        assert_eq!(t("---\ntags: \"#\"\n---\n"), Vec::<String>::new());
        assert_eq!(t("---\ntags:\n---\n"), Vec::<String>::new());
        assert_eq!(t("---\ntitle: x\n---\n"), Vec::<String>::new());
        assert_eq!(t("plain"), Vec::<String>::new());
    }

    #[test]
    fn strips_frontmatter() {
        assert_eq!(strip_frontmatter("---\ntitle: x\n---\nbody"), "body");
        assert_eq!(strip_frontmatter("no fm"), "no fm");
        let bad = "---\ntitle: [unclosed\n---\nbody";
        assert_eq!(strip_frontmatter(bad), bad);
    }

    #[test]
    fn malformed_frontmatter_is_ignored() {
        // Invalid YAML: whole content is body, so the H1 inside it is still found.
        let m = derive("---\ntitle: [unclosed\n---\n# Body Title\n");
        assert_eq!(m.title, "Body Title");
        assert_eq!(m.project, None);
        assert!(m.tags.is_empty());
        // Not a mapping.
        let m = derive("---\n- a\n- b\n---\n");
        assert_eq!(m.title, "Design Doc");
        // Unclosed frontmatter.
        let m = derive("---\ntitle: x\n# H\n");
        assert_eq!(m.title, "H");
        // Duplicate keys are a YAML error.
        let m = derive("---\ntitle: a\ntitle: b\n---\n");
        assert_eq!(m.title, "Design Doc");
    }

    #[test]
    fn full_example() {
        let m = derive_metadata(
            "Projects/Hades/Design.md",
            "---\ntitle: Design\nproject: hades\ntags: [arch]\n---\n# Ignored\nText\n",
        );
        assert_eq!(
            m,
            DerivedMeta {
                title: "Design".into(),
                project: Some("hades".into()),
                tags: vec!["arch".into()],
            }
        );
    }

    #[test]
    fn derived_values_are_capped() {
        let long = "x".repeat(MAX_TITLE_CHARS + 10);
        assert_eq!(
            derive(&format!("# {long}\n")).title.chars().count(),
            MAX_TITLE_CHARS
        );
        let many: Vec<String> = (0..MAX_TAGS + 5).map(|i| format!("t{i}")).collect();
        let m = derive(&format!("---\ntags: [{}]\n---\n", many.join(", ")));
        assert_eq!(m.tags.len(), MAX_TAGS);
        assert_eq!(truncate_chars("ééé", 2), "éé");
    }
}
