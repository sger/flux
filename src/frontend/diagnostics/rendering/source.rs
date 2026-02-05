//! Source code rendering utilities for diagnostics
//!
//! This module provides functions for rendering source code snippets with error highlighting,
//! including carets, labels, and inline suggestions.

use super::colors::Colors;
use crate::frontend::diagnostics::types::{Label, LabelStyle};
use crate::frontend::position::Span;

/// Sentinel value for end-of-line positions.
const END_OF_LINE_SENTINEL: usize = usize::MAX - 1;

/// Get a specific line from source code (1-indexed)
///
/// Returns `None` if the line number is 0 or exceeds the number of lines in the source.
pub fn get_source_line(source: &str, line: usize) -> Option<&str> {
    if line == 0 {
        return None;
    }

    source.lines().nth(line.saturating_sub(1))
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
            let line_len = line_text.len();

            // Print the source line
            out.push_str(&format!(
                "{:>width$} | {}\n",
                line_no,
                line_text,
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

    for line_no in start_line..=end_line {
        if let Some(line_text) = source.and_then(|src| get_source_line(src, line_no)) {
            let line_len = line_text.len();
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
                line_text,
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
