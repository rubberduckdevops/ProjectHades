//! Splits a note into chunks for embedding (see "Chunking (indexer)" in API_CONTRACT.md).
//!
//! 1. Strip the frontmatter.
//! 2. Split on ATX headings (`#`..`######`) outside fenced code blocks. Each section's heading
//!    is its heading path joined with ` > `; text before the first heading uses the note title.
//! 3. Merge sections under `MERGE_BELOW_CHARS` into the next one (empty sections are dropped).
//! 4. Split sections over `MAX_CHUNK_CHARS` on blank lines, else whitespace, else hard (on
//!    char boundaries), with `OVERLAP_CHARS` of overlap between consecutive pieces.
//!
//! All sizes are in characters, not bytes.

use crate::notes::{fence_marker, strip_frontmatter};

pub const MERGE_BELOW_CHARS: usize = 200;
pub const MAX_CHUNK_CHARS: usize = 2800;
pub const OVERLAP_CHARS: usize = 300;
pub const SNIPPET_CHARS: usize = 300;
const HEADING_SEPARATOR: &str = " > ";

/// One piece of a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Heading path, e.g. `Design > Storage` (the note title for text before any heading).
    pub heading: String,
    /// The chunk text (trimmed, never empty).
    pub text: String,
}

impl Chunk {
    /// The text that gets embedded: `"<title>\n<heading>\n\n<chunk text>"`.
    pub fn embed_text(&self, title: &str) -> String {
        format!("{title}\n{}\n\n{}", self.heading, self.text)
    }

    /// The first `SNIPPET_CHARS` characters with whitespace collapsed (`…` if truncated).
    pub fn snippet(&self) -> String {
        snippet(&self.text)
    }
}

/// Whitespace-collapsed prefix of `text`, at most `SNIPPET_CHARS` characters plus `…`.
pub fn snippet(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.char_indices().nth(SNIPPET_CHARS) {
        Some((i, _)) => format!("{}…", collapsed[..i].trim_end()),
        None => collapsed,
    }
}

/// An ATX heading line: `(level, text)`. Allows up to 3 spaces of indent and an optional
/// closing `#` sequence.
fn atx_heading(line: &str) -> Option<(usize, String)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let t = &line[indent..];
    let level = t.len() - t.trim_start_matches('#').len();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let rest = rest.trim();
    let stripped = rest.trim_end_matches('#');
    let text = if stripped.len() < rest.len()
        && (stripped.is_empty() || stripped.ends_with(char::is_whitespace))
    {
        stripped.trim_end()
    } else {
        rest
    };
    Some((level, text.to_owned()))
}

#[derive(Debug)]
struct Section {
    heading: String,
    /// The original heading line (kept as text when this section is merged into a previous one).
    heading_line: Option<String>,
    body: String,
}

/// Split the body into heading sections (the first one is the preamble).
fn sections(title: &str, body: &str) -> Vec<Section> {
    let mut out = vec![Section {
        heading: title.to_owned(),
        heading_line: None,
        body: String::new(),
    }];
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut fence: Option<char> = None;
    for line in body.lines() {
        if let Some(c) = fence_marker(line) {
            match fence {
                None => fence = Some(c),
                Some(open) if open == c => fence = None,
                Some(_) => {}
            }
        } else if fence.is_none()
            && let Some((level, text)) = atx_heading(line)
        {
            while stack.last().is_some_and(|(l, _)| *l >= level) {
                stack.pop();
            }
            stack.push((level, text));
            let path: Vec<&str> = stack
                .iter()
                .map(|(_, t)| t.as_str())
                .filter(|t| !t.is_empty())
                .collect();
            out.push(Section {
                heading: if path.is_empty() {
                    title.to_owned()
                } else {
                    path.join(HEADING_SEPARATOR)
                },
                heading_line: Some(line.trim().to_owned()),
                body: String::new(),
            });
            continue;
        }
        let cur = out.last_mut().expect("at least the preamble");
        cur.body.push_str(line);
        cur.body.push('\n');
    }
    out
}

/// Drop empty sections and merge small ones into the next section.
fn merge_small(sections: Vec<Section>) -> Vec<Section> {
    let mut out = Vec::new();
    let mut acc: Option<Section> = None;
    for sec in sections {
        acc = Some(match acc.take() {
            None => sec,
            Some(a) if a.body.trim().is_empty() => sec,
            Some(a) if a.body.trim().chars().count() < MERGE_BELOW_CHARS => {
                let mut body = a.body.trim_end().to_owned();
                body.push_str("\n\n");
                if let Some(h) = &sec.heading_line {
                    body.push_str(h);
                    body.push('\n');
                }
                body.push_str(&sec.body);
                Section { body, ..a }
            }
            Some(a) => {
                out.push(a);
                sec
            }
        });
    }
    out.extend(acc.filter(|a| !a.body.trim().is_empty()));
    out
}

/// End index (exclusive) of the last blank line ending in `(min, max]`, if any.
fn last_paragraph_break(chars: &[char], start: usize, min: usize, max: usize) -> Option<usize> {
    (min + 1..=max).rev().find(|&i| {
        if chars[i - 1] != '\n' {
            return false;
        }
        let mut j = i - 1;
        while j > start && matches!(chars[j - 1], ' ' | '\t' | '\r') {
            j -= 1;
        }
        j > start && chars[j - 1] == '\n'
    })
}

/// End index (exclusive) just after the last whitespace in `(min, max]`, if any.
fn last_whitespace(chars: &[char], min: usize, max: usize) -> Option<usize> {
    (min + 1..=max)
        .rev()
        .find(|&i| chars[i - 1].is_whitespace())
}

/// Split text longer than `MAX_CHUNK_CHARS` into overlapping pieces.
fn split_long(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= MAX_CHUNK_CHARS {
        return vec![text.to_owned()];
    }
    let piece = |a: usize, b: usize| chars[a..b].iter().collect::<String>().trim().to_owned();
    let mut pieces = Vec::new();
    let mut start = 0;
    loop {
        if chars.len() - start <= MAX_CHUNK_CHARS {
            pieces.push(piece(start, chars.len()));
            break;
        }
        let max = start + MAX_CHUNK_CHARS;
        let min = start + MAX_CHUNK_CHARS / 2;
        let end = last_paragraph_break(&chars, start, min, max)
            .or_else(|| last_whitespace(&chars, min, max))
            .unwrap_or(max);
        pieces.push(piece(start, end));
        // Step back for the overlap, then forward to the next word start (if one is close).
        let back = end - OVERLAP_CHARS;
        start = (back..(back + 50).min(end))
            .find(|&k| k > 0 && chars[k - 1].is_whitespace())
            .unwrap_or(back);
    }
    pieces.retain(|p| !p.is_empty());
    pieces
}

/// Chunk a note. `title` is the note's derived title. Empty notes give no chunks.
pub fn chunk_note(title: &str, content: &str) -> Vec<Chunk> {
    let body = strip_frontmatter(content);
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    merge_small(sections(title, body))
        .into_iter()
        .flat_map(|sec| {
            let heading = sec.heading;
            split_long(sec.body.trim())
                .into_iter()
                .map(move |text| Chunk {
                    heading: heading.clone(),
                    text,
                })
        })
        .filter(|c| !c.text.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headings(chunks: &[Chunk]) -> Vec<&str> {
        chunks.iter().map(|c| c.heading.as_str()).collect()
    }

    fn para(n: usize, word: &str) -> String {
        vec![word; n].join(" ")
    }

    #[test]
    fn empty_notes_have_no_chunks() {
        assert!(chunk_note("T", "").is_empty());
        assert!(chunk_note("T", "  \n\t\n").is_empty());
        assert!(chunk_note("T", "---\ntitle: x\n---\n").is_empty());
        assert!(chunk_note("T", "---\ntitle: x\n---\n\n   \n").is_empty());
        // Headings alone carry no text.
        assert!(chunk_note("T", "# Title\n## Sub\n").is_empty());
    }

    #[test]
    fn frontmatter_is_stripped() {
        let c = chunk_note("T", "---\ntitle: Secret FM\ntags: [a]\n---\nHello world");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "Hello world");
        assert_eq!(c[0].heading, "T");
        // Malformed frontmatter is body.
        let c = chunk_note("T", "---\ntitle: [x\n---\nHello");
        assert!(c[0].text.starts_with("---"));
    }

    #[test]
    fn heading_paths() {
        let long = para(60, "word");
        let md = format!(
            "intro {long}\n# Design\n{long}\n## Storage\n{long}\n### S3\n{long}\n## API\n{long}\n# Other\n{long}\n"
        );
        let c = chunk_note("Note", &md);
        assert_eq!(
            headings(&c),
            [
                "Note",
                "Design",
                "Design > Storage",
                "Design > Storage > S3",
                "Design > API",
                "Other"
            ]
        );
        assert!(c.iter().all(|c| !c.text.starts_with('#')));
    }

    #[test]
    fn heading_syntax() {
        assert_eq!(atx_heading("# A"), Some((1, "A".into())));
        assert_eq!(atx_heading("###### F ##"), Some((6, "F".into())));
        assert_eq!(atx_heading("   ## Indented"), Some((2, "Indented".into())));
        assert_eq!(atx_heading("## C# notes"), Some((2, "C# notes".into())));
        assert_eq!(atx_heading("#"), Some((1, String::new())));
        assert_eq!(atx_heading("#\tTab"), Some((1, "Tab".into())));
        assert_eq!(atx_heading("    # code"), None);
        assert_eq!(atx_heading("####### seven"), None);
        assert_eq!(atx_heading("#hashtag"), None);
        assert_eq!(atx_heading("text # not"), None);
    }

    #[test]
    fn headings_in_code_fences_are_ignored() {
        let long = para(60, "word");
        let md = format!("# Real\n{long}\n```bash\n# comment\n## also\n```\n~~~\n# x\n~~~\n");
        let c = chunk_note("T", &md);
        assert_eq!(headings(&c), ["Real"]);
        assert!(c[0].text.contains("# comment"));
        assert!(c[0].text.contains("~~~\n# x\n~~~"));
    }

    #[test]
    fn small_sections_merge_forward() {
        let long = para(60, "word");
        let md = format!("Short intro.\n# A\ntiny\n# B\n{long}\n# C\n{long}\n");
        let c = chunk_note("T", &md);
        // Preamble (small) absorbs A (still small) and then B.
        assert_eq!(headings(&c), ["T", "C"]);
        assert!(c[0].text.starts_with("Short intro."));
        assert!(
            c[0].text.contains("\n\n# A\ntiny\n\n# B\nword"),
            "{}",
            c[0].text
        );
    }

    #[test]
    fn empty_sections_are_dropped_not_merged() {
        let long = para(60, "word");
        let md = format!("# Design\n## Storage\n{long}\n");
        let c = chunk_note("T", &md);
        assert_eq!(headings(&c), ["Design > Storage"]);
        assert_eq!(c[0].text, long);
    }

    #[test]
    fn small_trailing_section_stays() {
        let long = para(60, "word");
        let c = chunk_note("T", &format!("# A\n{long}\n# B\nbye\n"));
        assert_eq!(headings(&c), ["A", "B"]);
        assert_eq!(c[1].text, "bye");
    }

    #[test]
    fn long_sections_split_on_blank_lines_with_overlap() {
        // 10 paragraphs of ~500 chars.
        let paras: Vec<String> = (0..10).map(|i| para(100, &format!("p{i}x"))).collect();
        let md = format!("# Big\n{}", paras.join("\n\n"));
        let c = chunk_note("T", &md);
        assert!(c.len() >= 2, "{}", c.len());
        for ch in &c {
            assert_eq!(ch.heading, "Big");
            assert!(ch.text.chars().count() <= MAX_CHUNK_CHARS);
        }
        // The first piece ends at a paragraph boundary.
        assert!(c[0].text.ends_with("x"));
        let first_last_para = c[0].text.rsplit("\n\n").next().unwrap();
        assert!(paras.contains(&first_last_para.to_owned()));
        // Consecutive pieces overlap by roughly OVERLAP_CHARS.
        for w in c.windows(2) {
            let tail: String = {
                let t: Vec<char> = w[0].text.chars().collect();
                t[t.len() - 100..].iter().collect()
            };
            assert!(w[1].text.contains(&tail), "overlap missing");
        }
        // Nothing is lost: every paragraph appears somewhere.
        for p in &paras {
            assert!(c.iter().any(|ch| ch.text.contains(p.as_str())));
        }
    }

    #[test]
    fn no_whitespace_text_is_hard_wrapped_on_char_boundaries() {
        let blob = "é".repeat(7000);
        let c = chunk_note("T", &blob);
        assert!(c.len() >= 3);
        let mut total = 0;
        for ch in &c {
            let n = ch.text.chars().count();
            assert!(n <= MAX_CHUNK_CHARS);
            total += n;
        }
        assert_eq!(total, 7000 + (c.len() - 1) * OVERLAP_CHARS);
    }

    #[test]
    fn single_long_paragraph_wraps_at_whitespace() {
        let text = para(1500, "lorem"); // 8999 chars, no blank lines
        let c = chunk_note("T", &text);
        assert!(c.len() >= 4);
        for ch in &c {
            assert!(ch.text.chars().count() <= MAX_CHUNK_CHARS);
            assert!(ch.text.starts_with("lorem") && ch.text.ends_with("lorem"));
        }
    }

    #[test]
    fn embed_text_and_snippet() {
        let ch = Chunk {
            heading: "Design > Storage".into(),
            text: "Content   lives\n\nin S3.".into(),
        };
        assert_eq!(
            ch.embed_text("Design"),
            "Design\nDesign > Storage\n\nContent   lives\n\nin S3."
        );
        assert_eq!(ch.snippet(), "Content lives in S3.");
        let long = Chunk {
            heading: "h".into(),
            text: "ab ".repeat(200),
        };
        let s = long.snippet();
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= SNIPPET_CHARS + 1);
        assert_eq!(
            snippet(&"日".repeat(400)).chars().count(),
            SNIPPET_CHARS + 1
        );
    }

    #[test]
    fn crlf_is_handled() {
        let long = para(60, "word");
        let md = format!("# A\r\n{long}\r\n## B\r\n{long}\r\n");
        assert_eq!(headings(&chunk_note("T", &md)), ["A", "A > B"]);
    }

    #[test]
    fn preamble_uses_title_and_blank_heading_falls_back() {
        let long = para(60, "word");
        let c = chunk_note("My Note", &format!("{long}\n#\n{long}\n"));
        assert_eq!(headings(&c), ["My Note", "My Note"]);
    }
}
