use crate::frontend::position::{Position, Span};
use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub title: String,
    pub code: Option<String>,
    pub message: Option<String>,
    pub file: Option<String>,
    pub position: Option<Position>,
    pub span: Option<Span>,
    pub hints: Vec<String>,
}

// ICE = Internal Compiler Error (a compiler bug, not user code).
#[macro_export]
macro_rules! ice {
    ($msg:expr) => {
        $crate::frontend::diagnostic::Diagnostic::error("INTERNAL COMPILER ERROR")
            .with_message($msg)
            .with_hint(format!("{}:{} ({})", file!(), line!(), module_path!()))
    };
}

impl Diagnostic {
    pub fn error(title: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            title: title.into(),
            code: None,
            message: None,
            file: None,
            position: None,
            span: None,
            hints: Vec::new(),
        }
    }

    pub fn warning(title: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            title: title.into(),
            code: None,
            message: None,
            file: None,
            position: None,
            span: None,
            hints: Vec::new(),
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn with_position(mut self, position: Position) -> Self {
        self.position = Some(position);
        self.span = Some(Span::new(position, position));
        self
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.position = Some(span.start);
        self.span = Some(span);
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    pub fn render(&self, source: Option<&str>, default_file: Option<&str>) -> String {
        let mut out = String::new();
        let use_color = env::var_os("NO_COLOR").is_none();
        let yellow = "\u{1b}[33m";
        let red = "\u{1b}[31m";
        let reset = "\u{1b}[0m";
        let file = self
            .file
            .as_deref()
            .or(default_file)
            .map(render_display_path)
            .unwrap_or_else(|| "<unknown>".to_string());
        let code = self.code.as_deref().unwrap_or("E000");

        if use_color {
            out.push_str(yellow);
        }
        out.push_str(&format!("-- {} -- {} -- [{}]\n", self.title, file, code));
        if use_color {
            out.push_str(reset);
        }

        if let Some(message) = &self.message {
            out.push('\n');
            out.push_str(message);
            out.push('\n');
        }

        if let Some(position) = self.position {
            let _display_column = position.column + 1;

            let span = self.span.unwrap_or_else(|| Span::new(position, position));
            let start_line = span.start.line;
            let end_line = span.end.line.max(start_line);
            let line_width = end_line.to_string().len();

            for line_no in start_line..=end_line {
                if let Some(line_text) = source.and_then(|src| get_source_line(src, line_no)) {
                    let line_len = line_text.len();
                    let (caret_start, caret_end) = if line_no == start_line && line_no == end_line
                    {
                        let start = span.start.column.min(line_len);
                        let end = span.end.column.min(line_len);
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

                    out.push('\n');
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
                        out.push_str(red);
                    }
                    let caret_len = caret_end.saturating_sub(caret_start).max(1);
                    out.push_str(&"^".repeat(caret_len));
                    if use_color {
                        out.push_str(reset);
                    }
                }
            }
        }

        if !self.hints.is_empty() {
            out.push('\n');
            for hint in &self.hints {
                out.push_str(&format!("\nHint: {}", hint));
            }
        }

        out
    }
}

pub fn render_diagnostics(
    diagnostics: &[Diagnostic],
    source: Option<&str>,
    default_file: Option<&str>,
) -> String {
    diagnostics
        .iter()
        .map(|diag| diag.render(source, default_file))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn get_source_line(source: &str, line: usize) -> Option<&str> {
    if line == 0 {
        return None;
    }

    source.lines().nth(line.saturating_sub(1))
}

fn render_display_path(file: &str) -> String {
    let path = std::path::Path::new(file);
    if path.is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(stripped) = path.strip_prefix(&cwd) {
                return stripped.to_string_lossy().to_string();
            }
        }
    }
    file.to_string()
}
