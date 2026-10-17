//! `docs/known_issues.md`, read for the census: which KIs are open, and which
//! fixture files each entry cites.

use std::collections::BTreeMap;

/// One `### KI-nnn — title` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub id: String,
    pub title: String,
    pub open: bool,
    /// The `.flx` paths the entry mentions, under `examples/`, `tests/` or
    /// `lib/`.
    pub files: Vec<String>,
}

/// The issues and, for each cited file, the issues that cite it.
#[derive(Debug, Default)]
pub struct KnownIssues {
    pub issues: Vec<Issue>,
    by_file: BTreeMap<String, Vec<usize>>,
}

impl KnownIssues {
    pub fn parse(text: &str) -> Self {
        let issues = parse_issues(text);
        let mut by_file: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, issue) in issues.iter().enumerate() {
            for file in &issue.files {
                by_file.entry(file.clone()).or_default().push(index);
            }
        }
        Self { issues, by_file }
    }

    pub fn get(&self, id: &str) -> Option<&Issue> {
        self.issues.iter().find(|issue| issue.id == id)
    }

    /// The issues whose entries cite `file`.
    pub fn citing(&self, file: &str) -> impl Iterator<Item = &Issue> {
        self.by_file
            .get(file)
            .into_iter()
            .flatten()
            .map(|&index| &self.issues[index])
    }
}

/// An entry is resolved when it sits under `## Resolved`, or when its heading
/// says `FIXED` or `NOT REPRODUCIBLE`, which some entries under `## Open` do.
fn parse_issues(text: &str) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let mut in_resolved = false;
    for line in text.lines() {
        if let Some(section) = line.strip_prefix("## ") {
            in_resolved = section.trim().starts_with("Resolved");
            continue;
        }
        if let Some(heading) = line.strip_prefix("### ")
            && heading.starts_with("KI-")
        {
            let id: String = heading
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            let title = heading[id.len()..]
                .trim_start_matches([' ', '—', '-'])
                .to_string();
            let upper = heading.to_uppercase();
            let resolved =
                in_resolved || upper.contains("FIXED") || upper.contains("NOT REPRODUCIBLE");
            issues.push(Issue {
                id,
                title,
                open: !resolved,
                files: Vec::new(),
            });
            continue;
        }
        if let Some(issue) = issues.last_mut() {
            for file in cited_files(line) {
                if !issue.files.contains(&file) {
                    issue.files.push(file);
                }
            }
        }
    }
    issues
}

/// The corpus paths in `line`: each run of path characters ending in `.flx`
/// that starts with `examples/`, `tests/` or `lib/`.
fn cited_files(line: &str) -> Vec<String> {
    let is_path_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-');
    let mut files = Vec::new();
    for (end, _) in line.match_indices(".flx") {
        let end = end + ".flx".len();
        let start = line[..end]
            .char_indices()
            .rev()
            .take_while(|&(_, c)| is_path_char(c))
            .last()
            .map_or(end, |(at, _)| at);
        // Markdown links from docs/ reach the corpus as `../examples/…`.
        let path = line[start..end].trim_start_matches(['.', '/']);
        if ["examples/", "tests/", "lib/"]
            .iter()
            .any(|root| path.starts_with(root))
        {
            files.push(path.to_string());
        }
    }
    files
}

/// The first `KI-nnn` in `text`.
pub fn ki_in(text: &str) -> Option<String> {
    let at = text.find("KI-")?;
    let digits: String = text[at + 3..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (!digits.is_empty()).then(|| format!("KI-{digits}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "\
# Known issues
## Open
### KI-102 — Effects leak under --no-cache
**Severity:** Medium · **Area:** VM
Fixture: `examples/type_classes/console_logger_example.flx` and
tests/parity/foo.flx:12, and [c](../examples/c.flx).
### KI-011 — Old bug — FIXED 2026-01-02
See examples/a.flx.
## Resolved
### KI-038 — Resolved without a marker
Was in ./examples/b.flx
";

    #[test]
    fn open_and_resolved_entries_with_their_files() {
        let known = KnownIssues::parse(DOC);
        let ids: Vec<(&str, bool)> = known
            .issues
            .iter()
            .map(|i| (i.id.as_str(), i.open))
            .collect();
        assert_eq!(
            ids,
            [("KI-102", true), ("KI-011", false), ("KI-038", false)]
        );
        let ki102 = known.get("KI-102").expect("parsed");
        assert_eq!(ki102.title, "Effects leak under --no-cache");
        assert_eq!(
            ki102.files,
            [
                "examples/type_classes/console_logger_example.flx",
                "tests/parity/foo.flx",
                "examples/c.flx"
            ]
        );
        let citing: Vec<&str> = known
            .citing("examples/b.flx")
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(citing, ["KI-038"]);
    }

    #[test]
    fn a_ki_is_found_inside_a_reason() {
        assert_eq!(
            ki_in("skip-way: KI-104 dictionary"),
            Some("KI-104".to_string())
        );
        assert_eq!(ki_in("no issue here"), None);
        assert_eq!(ki_in("KI-"), None);
    }
}
