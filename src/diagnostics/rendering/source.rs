//! Source code rendering utilities for diagnostics
//!
//! This module provides functions for rendering source code snippets with error highlighting,
//! including carets, labels, and inline suggestions.

use super::colors::Colors;
use crate::diagnostics::position::Span;
use crate::diagnostics::types::{Label, LabelStyle};

/// Sentinel value for end-of-line positions.
const END_OF_LINE_SENTINEL: usize = usize::MAX - 1;

/// Find the byte offset where a real comment starts outside of string literals.
///
/// Scans the line left-to-right, tracking `"..."` strings (with `\"` escape
/// handling). Returns `Some(offset)` at the first `//` or `/*` that is not
/// inside a string literal, or `None` if no such comment exists.
fn find_comment_start(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_string = false;

    while i < len {
        if in_string {
            match bytes[i] {
                b'\\' => {
                    // Skip escaped character (e.g. \", \\)
                    i += 2;
                }
                b'"' => {
                    in_string = false;
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            }
        } else {
            match bytes[i] {
                b'"' => {
                    in_string = true;
                    i += 1;
                }
                b'/' if i + 1 < len && (bytes[i + 1] == b'/' || bytes[i + 1] == b'*') => {
                    return Some(i);
                }
                _ => {
                    i += 1;
                }
            }
        }
    }

    None
}

/// Prepare a source line for diagnostic display by trimming comments that
/// fall outside the highlighted span.
///
/// Returns `(displayed_line, adjusted_col_start, adjusted_col_end)`.
///
/// - If the span overlaps the comment region the line is returned unchanged.
/// - Otherwise the comment (and trailing whitespace before it) is removed and
///   caret columns are clamped to the trimmed length.
pub fn render_diagnostic_line(
    source_line: &str,
    span_col_start: usize,
    span_col_end: usize,
) -> (String, usize, usize) {
    if let Some(comment_start) = find_comment_start(source_line) {
        // If the span extends into or past the comment, preserve the line
        if span_col_end > comment_start {
            return (source_line.to_string(), span_col_start, span_col_end);
        }

        // Trim comment and trailing whitespace
        let trimmed = source_line[..comment_start].trim_end();
        let trimmed_len = trimmed.len();

        let adj_start = span_col_start.min(trimmed_len);
        let adj_end = span_col_end.min(trimmed_len);

        (trimmed.to_string(), adj_start, adj_end)
    } else {
        (source_line.to_string(), span_col_start, span_col_end)
    }
}

/// Get a specific line from source code (1-indexed)
///
/// Returns `None` if the line number is 0 or exceeds the number of lines in the source.
pub fn get_source_line(source: &str, line: usize) -> Option<&str> {
    if line == 0 {
        return None;
    }

    source.lines().nth(line.saturating_sub(1))
}

/// Compute the maximum span column end that touches `line_no`, considering
/// the primary span and all labels. Used to decide whether comment trimming
/// is safe (i.e. no highlighted region extends into the comment).
fn max_span_col_on_line(line_no: usize, span: Span, labels: &[Label]) -> usize {
    let start_line = span.start.line;
    let end_line = span.end.line.max(start_line);
    let mut max_col: usize = 0;

    if line_no >= start_line && line_no <= end_line {
        if line_no == end_line {
            max_col = max_col.max(span.end.column);
        } else {
            // Start line or intermediate line of a multi-line span:
            // the highlight extends to end-of-line, so never trim.
            return usize::MAX;
        }
    }

    for label in labels {
        if label.span.end.line == line_no {
            max_col = max_col.max(label.span.end.column);
        }
        // If the label starts on this line but ends on a later line,
        // it extends to end-of-line.
        if label.span.start.line == line_no && label.span.end.line > line_no {
            return usize::MAX;
        }
    }

    max_col
}

/// Render a source code snippet with the primary error span and labels
///
/// This function displays:
/// - Line numbers with source code
/// - Caret (^) highlighting under the primary error span
/// - Label annotations (primary, secondary, note) with their messages
///
/// # Parameters
/// - `out`: String buffer to append rendered output
/// - `source`: Optional source code text
/// - `span`: The primary error span to highlight
/// - `labels`: Additional labels to render on the source
/// - `use_color`: Whether to use ANSI color codes
pub fn render_source_snippet(
    out: &mut String,
    source: Option<&str>,
    span: Span,
    labels: &[Label],
    use_color: bool,
) {
    let colors = if use_color {
        Colors::with_color()
    } else {
        Colors::no_color()
    };

    let start_line = span.start.line;
    let end_line = span.end.line.max(start_line);

    // Expand range to include all label lines
    let label_start = labels
        .iter()
        .map(|l| l.span.start.line)
        .min()
        .unwrap_or(start_line);
    let label_end = labels
        .iter()
        .map(|l| l.span.end.line)
        .max()
        .unwrap_or(end_line);
    let actual_start = start_line.min(label_start);
    let actual_end = end_line.max(label_end);

    let line_width = actual_end.to_string().len();

    // Add separator line
    out.push_str(&format!("{:>width$} |\n", "", width = line_width));

    let mut printed_any = false;
    for line_no in actual_start..=actual_end {
        if let Some(line_text) = source.and_then(|src| get_source_line(src, line_no)) {
            if printed_any {
                out.push('\n');
            }
            printed_any = true;

            // Compute the maximum span column end on this line from the
            // primary span and all labels so we know whether any highlighted
            // region extends into a comment.
            let max_span_col = max_span_col_on_line(line_no, span, labels);

            let (display_line, _, _) = render_diagnostic_line(line_text, 0, max_span_col);
            let line_len = display_line.len();

            // Print the (possibly trimmed) source line
            out.push_str(&format!(
                "{:>width$} | {}\n",
                line_no,
                display_line,
                width = line_width
            ));

            // Only render primary caret if this line is in the primary span
            let render_primary = line_no >= start_line && line_no <= end_line;

            // Render primary caret if this line is in the primary span
            if render_primary {
                let (caret_start, caret_end) = if line_no == start_line && line_no == end_line {
                    // Handle end-of-line sentinel value
                    let start = if span.start.column >= END_OF_LINE_SENTINEL {
                        line_len
                    } else {
                        span.start.column.min(line_len)
                    };
                    let end = if span.end.column >= END_OF_LINE_SENTINEL {
                        line_len
                    } else {
                        span.end.column.min(line_len)
                    };
                    let end = end.max(start + 1);
                    (start, end)
                } else if line_no == start_line {
                    let start = span.start.column.min(line_len);
                    (start, line_len.max(start + 1))
                } else if line_no == end_line {
                    let end = span.end.column.min(line_len);
                    (0, end.max(1))
                } else {
                    (0, line_len.max(1))
                };

                out.push_str(&format!(
                    "{:>width$} | {}",
                    "",
                    " ".repeat(caret_start),
                    width = line_width
                ));
                if use_color {
                    out.push_str(colors.red);
                }
                let caret_len = caret_end.saturating_sub(caret_start).max(1);
                out.push_str(&"^".repeat(caret_len));
                if use_color {
                    out.push_str(colors.reset);
                }
            }

            // Render labels for this line
            for label in labels {
                // Only render labels that are on this specific line
                if label.span.start.line == line_no && label.span.end.line == line_no {
                    let label_start = label.span.start.column.min(line_len);
                    let label_end = label.span.end.column.min(line_len).max(label_start + 1);
                    let label_len = label_end.saturating_sub(label_start);

                    // Choose color based on label style
                    let color = match label.style {
                        LabelStyle::Primary => colors.red,
                        LabelStyle::Secondary => colors.blue,
                        LabelStyle::Note => colors.cyan,
                    };

                    out.push('\n');
                    out.push_str(&format!(
                        "{:>width$} | {}",
                        "",
                        " ".repeat(label_start),
                        width = line_width
                    ));
                    if use_color {
                        out.push_str(color);
                    }
                    out.push_str(&"-".repeat(label_len));
                    if !label.text.is_empty() {
                        out.push(' ');
                        out.push_str(&label.text);
                    }
                    if use_color {
                        out.push_str(colors.reset);
                    }
                }
            }
        }
    }
}

/// Render a source snippet for hints with caret highlighting
///
/// This is a simplified version of `render_source_snippet` used specifically for
/// hint annotations. It displays the source code with caret highlighting but without
/// additional labels.
///
/// # Parameters
/// - `out`: String buffer to append rendered output
/// - `source`: Optional source code text
/// - `span`: The span to highlight
/// - `use_color`: Whether to use ANSI color codes
pub fn render_hint_snippet(out: &mut String, source: Option<&str>, span: Span, use_color: bool) {
    let colors = if use_color {
        Colors::with_color()
    } else {
        Colors::no_color()
    };

    let start_line = span.start.line;
    let end_line = span.end.line.max(start_line);
    let line_width = end_line.to_string().len();

    // Add separator line
    out.push_str(&format!("{:>width$} |\n", "", width = line_width));

    let empty_labels: &[Label] = &[];
    for line_no in start_line..=end_line {
        if let Some(line_text) = source.and_then(|src| get_source_line(src, line_no)) {
            let max_col = max_span_col_on_line(line_no, span, empty_labels);
            let (display_line, _, _) = render_diagnostic_line(line_text, 0, max_col);
            let line_len = display_line.len();

            let (caret_start, caret_end) = if line_no == start_line && line_no == end_line {
                let start = if span.start.column >= END_OF_LINE_SENTINEL {
                    line_len
                } else {
                    span.start.column.min(line_len)
                };
                let end = if span.end.column >= END_OF_LINE_SENTINEL {
                    line_len
                } else {
                    span.end.column.min(line_len)
                };
                (start, end.max(start + 1))
            } else if line_no == start_line {
                let start = span.start.column.min(line_len);
                (start, line_len.max(start + 1))
            } else if line_no == end_line {
                let end = span.end.column.min(line_len);
                (0, end.max(1))
            } else {
                (0, line_len.max(1))
            };

            out.push_str(&format!(
                "{:>width$} | {}\n",
                line_no,
                display_line,
                width = line_width
            ));
            out.push_str(&format!(
                "{:>width$} | {}",
                "",
                " ".repeat(caret_start),
                width = line_width
            ));
            if use_color {
                out.push_str(colors.red);
            }
            let caret_len = caret_end.saturating_sub(caret_start).max(1);
            out.push_str(&"^".repeat(caret_len));
            if use_color {
                out.push_str(colors.reset);
            }

            if line_no < end_line {
                out.push('\n');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── find_comment_start ──────────────────────────────────────────

    #[test]
    fn no_comment() {
        assert_eq!(find_comment_start("let x = 1;"), None);
    }

    #[test]
    fn line_comment_after_code() {
        assert_eq!(find_comment_start("print(len(arr));      // 5"), Some(22));
    }

    #[test]
    fn block_comment_after_code() {
        assert_eq!(find_comment_start("x = 1; /* comment */"), Some(7));
    }

    #[test]
    fn slash_inside_string_is_not_comment() {
        // The "//" is inside a closed string, so no real comment.
        assert_eq!(find_comment_start(r#"print("//")"#), None);
    }

    #[test]
    fn slash_inside_string_with_trailing_comment() {
        assert_eq!(find_comment_start(r#"print("//"); // comment"#), Some(13));
    }

    #[test]
    fn escaped_quote_keeps_string_open() {
        // The \" doesn't end the string, so // is still inside.
        assert_eq!(find_comment_start(r#""hello \" // still in string""#), None);
    }

    #[test]
    fn unterminated_string_no_comment() {
        // Opening quote never closed – the // is inside the string.
        assert_eq!(
            find_comment_start(r#"let greeting = "hello  // No semicolon needed"#),
            None
        );
    }

    // ── render_diagnostic_line ──────────────────────────────────────

    #[test]
    fn case1_trailing_comment_trimmed() {
        let line = "print(len(arr));      // 5";
        let (display, start, end) = render_diagnostic_line(line, 0, 16);
        assert_eq!(display, "print(len(arr));");
        assert_eq!(start, 0);
        assert_eq!(end, 16);
    }

    #[test]
    fn case2_unterminated_string_preserved() {
        let line = r#"let greeting = "hello  // No semicolon needed"#;
        // Span covers from the opening quote (col 15) to end of line (col 46)
        let (display, start, end) = render_diagnostic_line(line, 15, 46);
        assert_eq!(display, line); // unchanged
        assert_eq!(start, 15);
        assert_eq!(end, 46);
    }

    #[test]
    fn case3_block_comment_trimmed() {
        let line = "x = 1; /* comment */";
        let (display, start, end) = render_diagnostic_line(line, 0, 6);
        assert_eq!(display, "x = 1;");
        assert_eq!(start, 0);
        assert_eq!(end, 6);
    }

    #[test]
    fn case4_slash_in_string_trim_real_comment() {
        let line = r#"print("//"); // comment"#;
        // Span covers 0..12 => the print("//"); part
        let (display, start, end) = render_diagnostic_line(line, 0, 12);
        assert_eq!(display, r#"print("//");"#);
        assert_eq!(start, 0);
        assert_eq!(end, 12);
    }

    #[test]
    fn case5_escaped_quote_no_trim() {
        let line = r#""hello \" // still in string""#;
        // Span covers the whole string literal
        let (display, _, _) = render_diagnostic_line(line, 0, 29);
        // No comment found outside string, so nothing to trim
        assert_eq!(display, line);
    }

    #[test]
    fn caret_clamped_when_past_trimmed_end() {
        let line = "x = 1;  // comment";
        // Span col_end is within the code portion
        let (display, start, end) = render_diagnostic_line(line, 4, 5);
        assert_eq!(display, "x = 1;");
        assert_eq!(start, 4);
        assert_eq!(end, 5);
    }

    #[test]
    fn span_extending_into_comment_preserves_line() {
        let line = "x = 1;  // comment";
        // Span extends past comment_start (8)
        let (display, start, end) = render_diagnostic_line(line, 4, 18);
        assert_eq!(display, line); // not trimmed
        assert_eq!(start, 4);
        assert_eq!(end, 18);
    }

    #[test]
    fn no_comment_returns_unchanged() {
        let line = "let x = 42;";
        let (display, start, end) = render_diagnostic_line(line, 4, 5);
        assert_eq!(display, line);
        assert_eq!(start, 4);
        assert_eq!(end, 5);
    }
}
