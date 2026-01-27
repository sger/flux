use crate::frontend::position::Position;
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
    pub hints: Vec<String>,
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
        let reset = "\u{1b}[0m";
        let file = self.file.as_deref().or(default_file).unwrap_or("<unknown>");
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

            if let Some(line_text) = source.and_then(|src| get_source_line(src, position.line)) {
                let line_str = position.line.to_string();
                let gutter_width = line_str.len();
                let caret_indent = position.column.min(line_text.len());
                out.push('\n');
                out.push_str(&format!(
                    "{:>width$} | {}\n",
                    position.line,
                    line_text,
                    width = gutter_width
                ));
                out.push_str(&format!(
                    "{:>width$} | {}",
                    "",
                    " ".repeat(caret_indent),
                    width = gutter_width
                ));
                if use_color {
                    out.push_str(yellow);
                }
                out.push('^');
                if use_color {
                    out.push_str(reset);
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
