//! Main rendering orchestration for diagnostics
//!
//! This module contains the core rendering functions that orchestrate the display of
//! diagnostics, including headers, messages, locations, source snippets, suggestions,
//! hints, and related diagnostics.

use crate::diagnostics::position::Span;
use crate::diagnostics::types::{
    Hint, HintChain, HintKind, InlineSuggestion, RelatedDiagnostic, RelatedKind, Severity,
    StackTraceFrame,
};
use std::borrow::Cow;
use std::collections::HashMap;

use super::colors::Colors;
use super::formatter::render_display_path;
use super::source::{get_source_line, render_hint_snippet as render_hint_snippet_internal};

/// Sentinel value for end-of-line positions.
const END_OF_LINE_SENTINEL: usize = usize::MAX - 1;

/// Render the header line containing severity, code, and title.
pub fn render_header(
    out: &mut String,
    severity: Severity,
    title: &str,
    display_title: Option<&str>,
    code: &str,
    _message: Option<&str>,
    use_color: bool,
) {
    let colors = if use_color {
        Colors::with_color()
    } else {
        Colors::no_color()
    };

    if use_color {
        out.push_str(colors.yellow);
    }
    let label = header_label(severity);
    let title_text = display_title_for_text(title, display_title);
    if code == "E000" && severity != Severity::Error {
        out.push_str(&format!("{label}: {title_text}"));
    } else {
        out.push_str(&format!("{label}[{code}]: {title_text}"));
    }
    if use_color {
        out.push_str(colors.reset);
    }
    out.push('\n');
}

/// Render the diagnostic message
///
/// Displays the main error message with appropriate spacing.
///
/// # Parameters
/// - `out`: String buffer to append rendered output
/// - `message`: Optional message text
pub fn render_message(out: &mut String, message: Option<&str>) {
    // Message
    if let Some(message) = message
        && !message.is_empty()
    {
        out.push('\n');
        out.push_str(message);
        out.push('\n');
        return;
    }
    // Keep a blank line between header and location when no message is provided.
    out.push('\n');
}

/// Render the source location line for a diagnostic when a span is available.
pub fn render_location(
    out: &mut String,
    source: Option<&str>,
    file: &str,
    span: Option<Span>,
    message: Option<&str>,
) {
    let colors = if std::env::var_os("NO_COLOR").is_none() {
        Colors::with_color()
    } else {
        Colors::no_color()
    };

    // Location indicator: --> file:line:column
    if let Some(span) = span {
        let position = span.start;
        // Only add newline if there was a non-empty message
        if message.is_some_and(|m| !m.is_empty()) {
            out.push('\n');
        }
        // Handle end-of-line sentinel value
        let display_col = if position.column >= END_OF_LINE_SENTINEL {
            // Get actual line length from source if available
            source
                .and_then(|src| get_source_line(src, position.line))
                .map(|line| line.len() + 1)
                .unwrap_or(1)
        } else {
            position.column + 1
        };
        out.push_str("  ");
        out.push_str(colors.dim);
        out.push_str(colors.cyan);
        out.push_str(&format!("{}:{}:{}", file, position.line, display_col));
        out.push_str(colors.reset);
        out.push('\n');
    }
}

/// Render inline code suggestions with tildes
///
/// Displays code suggestions showing the replaced line with tilde markers.
/// Format:
/// ```text
///    |
/// help: Replace with 'fn'
///    |
///  5 | fn add(a, b) {
///    | ~~~
/// ```
///
/// # Parameters
/// - `out`: String buffer to append rendered output
/// - `source`: Optional source code text
/// - `suggestions`: List of inline suggestions to render
/// - `use_color`: Whether to use ANSI color codes
pub fn render_suggestions(
    out: &mut String,
    source: Option<&str>,
    suggestions: &[InlineSuggestion],
    use_color: bool,
) {
    if suggestions.is_empty() {
        return;
    }

    let colors = if use_color {
        Colors::with_color()
    } else {
        Colors::no_color()
    };

    for suggestion in suggestions {
        let span = suggestion.span;
        let line_no = span.start.line;

        // Get the source line
        if let Some(line_text) = source.and_then(|src| get_source_line(src, line_no)) {
            let line_width = line_no.to_string().len();

            // Show "help:" message
            out.push_str("   |\n");
            if use_color {
                out.push_str(colors.green);
            }
            if let Some(msg) = &suggestion.message {
                out.push_str(&format!("help: {}\n", msg));
            } else {
                out.push_str(&format!(
                    "help: Replace with '{}'\n",
                    suggestion.replacement
                ));
            }
            if use_color {
                out.push_str(colors.reset);
            }

            // Show the line with replacement
            // Note: Use the same logic as render_source_snippet for consistency
            let start_col = if span.start.column >= END_OF_LINE_SENTINEL {
                line_text.len()
            } else {
                span.start.column.min(line_text.len())
            };
            let end_col = if span.end.column >= END_OF_LINE_SENTINEL {
                line_text.len()
            } else {
                span.end.column.min(line_text.len())
            };

            // Build the line with replacement
            let prefix = &line_text[..start_col];
            let suffix = &line_text[end_col..];
            let replaced_line = format!("{}{}{}", prefix, suggestion.replacement, suffix);

            out.push_str("   |\n");
            out.push_str(&format!(
                "{:>width$} | {}\n",
                line_no,
                replaced_line,
                width = line_width
            ));

            // Show tildes under the replacement
            out.push_str(&format!(
                "{:>width$} | {}",
                "",
                " ".repeat(start_col),
                width = line_width
            ));
            if use_color {
                out.push_str(colors.green);
            }
            out.push_str(&"~".repeat(suggestion.replacement.len()));
            if use_color {
                out.push_str(colors.reset);
            }
            out.push('\n');
        }
    }
}

/// Render all hints grouped by kind (text and span-based)
///
/// Displays hints in the following order:
/// 1. Text-only hints grouped by kind (Hint, Note, Help, Example)
/// 2. Span-based hints with source snippets
/// 3. Hint chains with step-by-step guidance
///
/// # Parameters
/// - `out`: String buffer to append rendered output
/// - `source`: Optional primary source code text
/// - `diagnostic_file`: The diagnostic's file path (for hint file fallback)
/// - `default_file`: Optional default file path
/// - `hints`: List of hints to render
/// - `hint_chains`: List of hint chains to render
/// - `sources_by_file`: Optional map of file paths to source code
/// - `use_color`: Whether to use ANSI color codes
#[allow(clippy::too_many_arguments)]
pub fn render_hints(
    out: &mut String,
    source: Option<&str>,
    diagnostic_file: Option<&str>,
    default_file: Option<&str>,
    hints: &[Hint],
    hint_chains: &[HintChain],
    sources_by_file: Option<&HashMap<String, String>>,
    use_color: bool,
) {
    if hints.is_empty() && hint_chains.is_empty() {
        return;
    }

    let colors = if use_color {
        Colors::with_color()
    } else {
        Colors::no_color()
    };

    // Separate hints into those with and without spans
    let (text_hints, span_hints): (Vec<_>, Vec<_>) = hints.iter().partition(|h| h.span.is_none());

    // Group text-only hints by kind
    let mut hints_by_kind: HashMap<HintKind, Vec<&Hint>> = HashMap::new();
    for hint in text_hints {
        hints_by_kind.entry(hint.kind).or_default().push(hint);
    }

    // Render text-only hints grouped by kind
    // Order: Hint, Note, Help, Example
    for kind in [
        HintKind::Hint,
        HintKind::Note,
        HintKind::Help,
        HintKind::Example,
    ] {
        if let Some(hints) = hints_by_kind.get(&kind) {
            ensure_section_spacing(out);
            let (label, color) = match kind {
                HintKind::Hint => ("Hint", colors.blue),
                HintKind::Note => ("Note", colors.cyan),
                HintKind::Help => ("Help", colors.green),
                HintKind::Example => ("Example", colors.blue),
            };
            if use_color {
                out.push_str(color);
            }
            out.push_str(&format!("{}:\n", label));
            if use_color {
                out.push_str(colors.reset);
            }
            for hint in hints {
                out.push_str(&format!("  {}\n", hint.text));
            }
        }
    }

    // Render hints with spans
    for hint in span_hints {
        if let Some(span) = hint.span {
            // Add separator before each span-based hint
            ensure_section_spacing(out);

            // Render the note header with optional label
            if let Some(label) = &hint.label {
                if use_color {
                    out.push_str(colors.blue);
                }
                out.push_str(&format!("   = note: {}\n", label));
                if use_color {
                    out.push_str(colors.reset);
                }
            } else {
                if use_color {
                    out.push_str(colors.blue);
                }
                out.push_str("   = note:\n");
                if use_color {
                    out.push_str(colors.reset);
                }
            }

            // Render location
            let start = span.start;
            let display_col = if start.column >= END_OF_LINE_SENTINEL {
                source
                    .and_then(|src| get_source_line(src, start.line))
                    .map(|line| line.len() + 1)
                    .unwrap_or(1)
            } else {
                start.column + 1
            };

            // Use hint's file if specified, otherwise fall back to diagnostic's file
            let file = hint
                .file
                .as_deref()
                .or(diagnostic_file)
                .filter(|f| !f.is_empty())
                .map(render_display_path)
                .unwrap_or_else(|| Cow::Borrowed("<unknown>"));
            out.push_str(&format!("  {}:{}:{}\n", file, start.line, display_col));

            // Render source snippet for this hint (use hint's file if specified)
            let hint_source = match hint.file.as_deref() {
                Some(file) => sources_by_file
                    .and_then(|sources| sources.get(file).map(|s| s.as_str()))
                    .or_else(|| {
                        if diagnostic_file == Some(file) || default_file == Some(file) {
                            source
                        } else {
                            None
                        }
                    }),
                None => source,
            };
            render_hint_snippet_internal(out, hint_source, span, use_color);

            // Render hint text if provided
            if !hint.text.is_empty() {
                let (label, color) = match hint.kind {
                    HintKind::Hint => ("Hint", colors.blue),
                    HintKind::Note => ("Note", colors.cyan),
                    HintKind::Help => ("Help", colors.green),
                    HintKind::Example => ("Example", colors.blue),
                };
                out.push_str("\n\n");
                if use_color {
                    out.push_str(color);
                }
                out.push_str(&format!("{}:\n", label));
                if use_color {
                    out.push_str(colors.reset);
                }
                out.push_str(&format!("  {}\n", hint.text));
            }
        }
    }

    // Render hint chains
    for chain in hint_chains {
        ensure_section_spacing(out);
        if use_color {
            out.push_str(colors.blue);
        }
        out.push_str("Hint:\n");
        if use_color {
            out.push_str(colors.reset);
        }
        out.push_str("  To fix this error:\n");

        for (i, step) in chain.steps.iter().enumerate() {
            out.push_str(&format!("    {}. {}\n", i + 1, step));
        }

        if let Some(conclusion) = &chain.conclusion {
            out.push_str(&format!("\n  {}\n", conclusion));
        }
    }
}

/// Render related diagnostics with cross-file references
///
/// Displays related diagnostic entries (note/help/related) that can reference
/// different files with their own source snippets.
///
/// # Parameters
/// - `out`: String buffer to append rendered output
/// - `source`: Optional primary source code text
/// - `diagnostic_file`: The diagnostic's file path (for related file fallback)
/// - `default_file`: Optional default file path
/// - `related`: List of related diagnostics to render
/// - `sources_by_file`: Optional map of file paths to source code
/// - `use_color`: Whether to use ANSI color codes
pub fn render_related(
    out: &mut String,
    source: Option<&str>,
    diagnostic_file: Option<&str>,
    default_file: Option<&str>,
    related: &[RelatedDiagnostic],
    sources_by_file: Option<&HashMap<String, String>>,
    use_color: bool,
) {
    if related.is_empty() {
        return;
    }

    let colors = if use_color {
        Colors::with_color()
    } else {
        Colors::no_color()
    };

    for related_item in related {
        ensure_section_spacing(out);
        let (label, color) = match related_item.kind {
            RelatedKind::Note => ("note", colors.cyan),
            RelatedKind::Help => ("help", colors.green),
            RelatedKind::Related => ("related", colors.blue),
        };
        if use_color {
            out.push_str(color);
        }
        if related_item.message.is_empty() {
            out.push_str(&format!("{}:", label));
        } else {
            out.push_str(&format!("{}: {}", label, related_item.message));
        }
        if use_color {
            out.push_str(colors.reset);
        }
        out.push('\n');

        if let Some(span) = related_item.span {
            let related_source = match related_item.file.as_deref() {
                Some(file) => sources_by_file
                    .and_then(|sources| sources.get(file).map(|s| s.as_str()))
                    .or_else(|| {
                        if diagnostic_file == Some(file) || default_file == Some(file) {
                            source
                        } else {
                            None
                        }
                    }),
                None => source,
            };
            let file = related_item
                .file
                .as_deref()
                .or(diagnostic_file)
                .or(default_file)
                .filter(|f| !f.is_empty())
                .map(render_display_path)
                .unwrap_or_else(|| Cow::Borrowed("<unknown>"));
            let start = span.start;
            let display_col = if start.column >= END_OF_LINE_SENTINEL {
                related_source
                    .and_then(|src| get_source_line(src, start.line))
                    .map(|line| line.len() + 1)
                    .unwrap_or(1)
            } else {
                start.column + 1
            };
            out.push_str(&format!("  {}:{}:{}\n", file, start.line, display_col));
            render_hint_snippet_internal(out, related_source, span, use_color);
        }
    }
}

/// Render a structured runtime stack trace.
pub fn render_stack_trace(out: &mut String, stack_trace: &[StackTraceFrame], use_color: bool) {
    if stack_trace.is_empty() {
        return;
    }

    let colors = if use_color {
        Colors::with_color()
    } else {
        Colors::no_color()
    };

    ensure_section_spacing(out);
    if use_color {
        out.push_str(colors.cyan);
    }
    out.push_str("Stack trace:\n");
    if use_color {
        out.push_str(colors.reset);
    }

    for frame in stack_trace {
        out.push_str("  at ");
        out.push_str(&frame.text);
        out.push('\n');
    }
}

fn ensure_section_spacing(out: &mut String) {
    if out.ends_with("\n\n") {
        return;
    }
    if out.ends_with('\n') {
        out.push('\n');
    } else {
        out.push_str("\n\n");
    }
}

fn header_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "Warning",
        Severity::Note => "Note",
        Severity::Help => "Help",
    }
}

fn display_title_for_text(title: &str, display_title: Option<&str>) -> String {
    display_title
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| humanize_title(title))
}

fn humanize_title(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return "Diagnostic".to_string();
    }
    if !trimmed
        .chars()
        .all(|ch| !ch.is_ascii_lowercase() || ch.is_whitespace())
    {
        return trimmed.to_string();
    }

    trimmed
        .split_whitespace()
        .map(humanize_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn humanize_word(word: &str) -> String {
    word.split('-')
        .map(|part| {
            if matches!(part, "ADT" | "IO" | "JIT" | "VM" | "EOF" | "API" | "JSON") {
                part.to_string()
            } else {
                title_case_segment(part)
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn title_case_segment(segment: &str) -> String {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    let mut out = String::new();
    out.extend(first.to_uppercase());
    for ch in chars {
        out.extend(ch.to_lowercase());
    }
    out
}
