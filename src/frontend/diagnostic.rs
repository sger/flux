use crate::frontend::position::Position;
use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Help,
}

impl Severity {
    fn as_str(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
            Severity::Help => "help",
        }
    }

    fn color_code(&self) -> &'static str {
        match self {
            Severity::Error => "\u{1b}[31m",   // red
            Severity::Warning => "\u{1b}[33m", // yellow
            Severity::Note => "\u{1b}[34m",    // blue
            Severity::Help => "\u{1b}[32m",    // green
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub title: String,
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
            message: None,
            file: None,
            position: None,
            hints: Vec::new(),
        }
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
        let reset = "\u{1b}[0m";
        if use_color {
            out.push_str(self.severity.color_code());
        }
        out.push_str(self.severity.as_str());
        if use_color {
            out.push_str(reset);
        }
        out.push_str(": ");
        out.push_str(&self.title);

        let file = self
            .file
            .as_deref()
            .or(default_file)
            .unwrap_or("<unknown>");

        if let Some(position) = self.position {
            let display_column = position.column + 1;
            out.push_str(&format!("\n --> {}:{}:{}", file, position.line, display_column));

            if let Some(line_text) = source.and_then(|src| get_source_line(src, position.line)) {
                let line_str = position.line.to_string();
                let gutter_width = line_str.len();
                let caret_indent = position.column.min(line_text.len());
                let label = self
                    .message
                    .as_deref()
                    .unwrap_or(&self.title);

                out.push_str("\n");
                out.push_str(&format!("{:>width$} | \n", "", width = gutter_width));
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
                    out.push_str(self.severity.color_code());
                }
                out.push_str("^");
                if use_color {
                    out.push_str(reset);
                }
                out.push_str(" ");
                out.push_str(label);
            } else if let Some(message) = &self.message {
                out.push_str(&format!("\n  = {}", message));
            }
        } else if let Some(message) = &self.message {
            out.push_str(&format!("\n  = {}", message));
        }

        if !self.hints.is_empty() {
            out.push_str("\n");
            for hint in &self.hints {
                out.push_str(&format!("\n= hint: {}", hint));
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
