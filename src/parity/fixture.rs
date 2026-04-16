//! Fixture metadata parsing for parity corpus files.
//!
//! Reads inline metadata from comment headers in `.flx` fixture files:
//!
//! ```text
//! // parity: vm, llvm
//! // expect: success
//! // bug: description of the bug shape
//! ```

use std::path::Path;

use super::Way;

/// Expected outcome for a fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    Success,
    CompileError,
    RuntimeError,
}

/// Parsed metadata from a parity fixture file.
#[derive(Debug)]
pub struct FixtureMeta {
    /// Which ways to compare (defaults to `[Vm, Llvm]`).
    pub ways: Vec<Way>,
    /// Expected outcome (defaults to `Success`).
    pub expect: Expect,
    /// One-line description of the bug shape.
    pub bug: Option<String>,
    /// Optional expected normalized stdout block.
    pub expected_stdout: Option<String>,
}

impl Default for FixtureMeta {
    fn default() -> Self {
        Self {
            ways: vec![Way::Vm, Way::Llvm],
            expect: Expect::Success,
            bug: None,
            expected_stdout: None,
        }
    }
}

/// Parse fixture metadata from inline comments at the top of a `.flx` file.
///
/// Returns `FixtureMeta::default()` if no metadata comments are found.
pub fn parse_fixture_meta(path: &Path) -> FixtureMeta {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return FixtureMeta::default(),
    };

    let mut meta = FixtureMeta::default();
    let mut collecting_expected_stdout = false;
    let mut expected_stdout_lines: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Stop at the first non-comment, non-empty line
        if !trimmed.is_empty() && !trimmed.starts_with("//") {
            break;
        }

        let Some(comment) = trimmed.strip_prefix("//") else {
            continue;
        };
        let comment_trimmed = comment.trim();

        if collecting_expected_stdout {
            if comment_trimmed == "parity-expected-stdout-end"
                || comment_trimmed == "parity-oracle-stdout-end"
            {
                collecting_expected_stdout = false;
                meta.expected_stdout = Some(expected_stdout_lines.join("\n").trim().to_string());
                expected_stdout_lines.clear();
                continue;
            }
            // Preserve internal whitespace for multi-line strings in expected
            // stdout. We only strip the `// ` prefix (one leading space), not
            // the rest of the content.
            let content_line = comment.strip_prefix(' ').unwrap_or(comment).to_string();
            expected_stdout_lines.push(content_line);
            continue;
        }
        let comment = comment_trimmed;

        if let Some(value) = comment.strip_prefix("parity:") {
            let value = value.trim();
            let ways: Vec<Way> = value
                .split(',')
                .filter_map(|s| Way::parse(s.trim()))
                .collect();
            if !ways.is_empty() {
                meta.ways = ways;
            }
        } else if let Some(value) = comment.strip_prefix("expect:") {
            let value = value.trim();
            meta.expect = match value {
                "success" => Expect::Success,
                "compile_error" => Expect::CompileError,
                "runtime_error" => Expect::RuntimeError,
                _ => Expect::Success,
            };
        } else if let Some(value) = comment.strip_prefix("bug:") {
            meta.bug = Some(value.trim().to_string());
        } else if comment == "parity-expected-stdout-begin"
            || comment == "parity-oracle-stdout-begin"
        {
            collecting_expected_stdout = true;
        }
    }

    if collecting_expected_stdout {
        meta.expected_stdout = Some(expected_stdout_lines.join("\n").trim().to_string());
    }

    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_full_metadata() {
        let dir = std::env::temp_dir().join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_fixture.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "// parity: vm, llvm").unwrap();
        writeln!(f, "// expect: runtime_error").unwrap();
        writeln!(f, "// bug: division by zero differs across backends").unwrap();
        writeln!(f, "// parity-expected-stdout-begin").unwrap();
        writeln!(f, "// line 1").unwrap();
        writeln!(f, "// line 2").unwrap();
        writeln!(f, "// parity-expected-stdout-end").unwrap();
        writeln!(f, "fn main() {{ }}").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(meta.ways, vec![Way::Vm, Way::Llvm]);
        assert_eq!(meta.expect, Expect::RuntimeError);
        assert_eq!(
            meta.bug.as_deref(),
            Some("division by zero differs across backends")
        );
        assert_eq!(meta.expected_stdout.as_deref(), Some("line 1\nline 2"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn defaults_without_metadata() {
        let dir = std::env::temp_dir().join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_no_meta.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "fn main() {{ }}").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(meta.ways, vec![Way::Vm, Way::Llvm]);
        assert_eq!(meta.expect, Expect::Success);
        assert!(meta.bug.is_none());
        assert!(meta.expected_stdout.is_none());

        let _ = std::fs::remove_file(&path);
    }
}
