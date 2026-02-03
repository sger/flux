use crate::frontend::position::{Position, Span};
use super::{ErrorCode, ErrorType, format_message};
use std::borrow::Cow;
use std::env;

// Error code constants for special rendering cases
const UNTERMINATED_STRING_ERROR_CODE: &str = "E031";
// Sentinel value for end-of-line positions.
const END_OF_LINE_SENTINEL: usize = usize::MAX - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    severity: Severity,
    title: String,
    code: Option<String>,
    error_type: Option<ErrorType>,
    message: Option<String>,
    file: Option<String>,
    span: Option<Span>,
    hints: Vec<String>,
}

// ICE = Internal Compiler Error (a compiler bug, not user code).
#[macro_export]
macro_rules! ice {
    ($msg:expr) => {
        $crate::frontend::diagnostics::Diagnostic::error("INTERNAL COMPILER ERROR")
            .with_message($msg)
            .with_hint(format!("{}:{} ({})", file!(), line!(), module_path!()))
    };
}

impl Diagnostic {
    /// Create a new error diagnostic with the given title.
    pub fn error(title: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            title: title.into(),
            code: None,
            error_type: None,
            message: None,
            file: None,
            span: None,
            hints: Vec::new(),
        }
    }

    /// Create a new warning diagnostic with the given title.
    pub fn warning(title: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            title: title.into(),
            code: None,
            error_type: None,
            message: None,
            file: None,
            span: None,
            hints: Vec::new(),
        }
    }

    /// Get the starting position from the span (derived field)
    pub fn position(&self) -> Option<Position> {
        self.span.map(|s| s.start)
    }

    // Getters for read access
    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub fn error_type(&self) -> Option<ErrorType> {
        self.error_type
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    pub fn span(&self) -> Option<Span> {
        self.span
    }

    pub fn hints(&self) -> &[String] {
        &self.hints
    }

    // Setter for file (needed by module_graph)
    pub fn set_file(&mut self, file: impl Into<String>) {
        self.file = Some(file.into());
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_error_type(mut self, error_type: ErrorType) -> Self {
        self.error_type = Some(error_type);
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
        self.span = Some(Span::new(position, position));
        self
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        let hint = hint.into();
        let cleaned = hint
            .strip_prefix("Hint:\n")
            .or_else(|| hint.strip_prefix("Hint:"))
            .unwrap_or(hint.as_str())
            .trim_start();
        self.hints.push(cleaned.to_string());
        self
    }

    /// Generic error builder using ErrorCode specification
    pub fn make_error(
        err_spec: &'static ErrorCode,
        values: &[&str],
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        let message = format_message(err_spec.message, values);
        let hint = err_spec.hint.map(|h| format_message(h, values));

        let mut diag = Diagnostic::error(err_spec.title)
            .with_code(err_spec.code)
            .with_error_type(err_spec.error_type)
            .with_file(file)
            .with_span(span)
            .with_message(message);

        if let Some(hint_text) = hint {
            diag = diag.with_hint(hint_text);
        }

        diag
    }

    /// Dynamic error builder for runtime-generated error information
    /// Use this when error details come from runtime values rather than static ErrorCode
    pub fn make_error_dynamic(
        code: impl Into<String>,
        title: impl Into<String>,
        error_type: ErrorType,
        message: impl Into<String>,
        hint: Option<String>,
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        let mut diag = Diagnostic::error(title)
            .with_code(code)
            .with_error_type(error_type)
            .with_file(file)
            .with_span(span)
            .with_message(message);

        if let Some(hint_text) = hint {
            diag = diag.with_hint(hint_text);
        }

        diag
    }

    /// Warning builder for linter and non-fatal issues
    pub fn make_warning(
        code: impl Into<String>,
        title: impl Into<String>,
        message: impl Into<String>,
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        Diagnostic::warning(title)
            .with_code(code)
            .with_file(file)
            .with_span(span)
            .with_message(message)
    }

    /// Note builder for informational diagnostics.
    pub fn make_note(
        title: impl Into<String>,
        message: impl Into<String>,
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        Self {
            severity: Severity::Note,
            title: title.into(),
            code: None,
            error_type: None,
            message: Some(message.into()),
            file: Some(file.into()),
            span: Some(span),
            hints: Vec::new(),
        }
    }

    /// Help builder for suggestions and assistance.
    pub fn make_help(
        title: impl Into<String>,
        message: impl Into<String>,
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        Self {
            severity: Severity::Help,
            title: title.into(),
            code: None,
            error_type: None,
            message: Some(message.into()),
            file: Some(file.into()),
            span: Some(span),
            hints: Vec::new(),
        }
    }

    fn render_header(
        &self,
        out: &mut String,
        use_color: bool,
        error_type_label: &str,
        code: &str,
    ) {
        let yellow = "\u{1b}[33m";
        let reset = "\u{1b}[0m";
        // Header: -- Compiler error: expected expression [E031]
        if use_color {
            out.push_str(yellow);
        }
        out.push_str(&format!("-- {}: {} [{}]\n", error_type_label, self.title, code));
        if use_color {
            out.push_str(reset);
        }
    }

    fn render_message(&self, out: &mut String) {
        // Message
        if let Some(message) = &self.message {
            out.push('\n');
            out.push_str(message);
            out.push('\n');
        }
    }

    fn render_location(
        &self,
        out: &mut String,
        source: Option<&str>,
        file: &str,
    ) {
        // Location indicator: --> file:line:column
        if let Some(position) = self.position() {
            out.push('\n');
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
            out.push_str(&format!(
                "  --> {}:{}:{}\n",
                file,
                position.line,
                display_col
            ));
        }
    }

    fn render_source_snippet(&self, out: &mut String, source: Option<&str>, use_color: bool) {
        let red = "\u{1b}[31m";
        let reset = "\u{1b}[0m";
        if let Some(position) = self.position() {
            let span = self.span.unwrap_or_else(|| Span::new(position, position));
            let start_line = span.start.line;
            let end_line = span.end.line.max(start_line);
            let line_width = end_line.to_string().len();

            // Add separator line
            out.push_str(&format!("{:>width$} |\n", "", width = line_width));

            let mut printed_any = false;
            for line_no in start_line..=end_line {
                if let Some(line_text) = source.and_then(|src| get_source_line(src, line_no)) {
                    if printed_any {
                        out.push('\n');
                    }
                    printed_any = true;
                    let line_len = line_text.len();
                    let mut caret_start;
                    let mut caret_end;
                    if line_no == start_line && line_no == end_line {
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
                        caret_start = start;
                        caret_end = end;
                    } else if line_no == start_line {
                        let start = span.start.column.min(line_len);
                        caret_start = start;
                        caret_end = line_len.max(start + 1);
                    } else if line_no == end_line {
                        let end = span.end.column.min(line_len);
                        caret_start = 0;
                        caret_end = end.max(1);
                    } else {
                        caret_start = 0;
                        caret_end = line_len.max(1);
                    }

                    // Special handling for unterminated string literals
                    // Highlight the opening quote instead of EOF position
                    if line_no == start_line
                        && line_no == end_line
                        && self.code.as_deref() == Some(UNTERMINATED_STRING_ERROR_CODE)
                        && self.message.as_deref().map_or(false, |msg| msg.contains("unterminated string"))
                    {
                        let start_col = span.start.column.min(line_len);
                        if let Some((quote_idx, _)) = line_text
                            .char_indices()
                            .find(|(idx, ch)| *idx >= start_col && *ch == '"')
                        {
                            caret_start = quote_idx;
                            caret_end = (quote_idx + 1).min(line_len.max(1));
                        }
                    }

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
    }

    fn render_hints(&self, out: &mut String) {
        // Hints section
        if !self.hints.is_empty() {
            out.push_str("\n\nHint:\n");
            for hint in &self.hints {
                out.push_str(&format!("  {}\n", hint));
            }
        }
    }

    pub fn render(&self, source: Option<&str>, default_file: Option<&str>) -> String {
        let mut out = String::new();
        let use_color = env::var_os("NO_COLOR").is_none();
        let file = self
            .file
            .as_deref()
            .filter(|f| !f.is_empty())
            .or(default_file)
            .map(render_display_path)
            .unwrap_or_else(|| Cow::Borrowed("<unknown>"));
        let code = self.code.as_deref().unwrap_or("E000");

        // Get error type prefix from explicit error_type field
        let error_type_label = self
            .error_type
            .map(|error_type| error_type.prefix())
            .unwrap_or("error");

        self.render_header(&mut out, use_color, error_type_label, code);
        self.render_message(&mut out);
        self.render_location(&mut out, source, file.as_ref());
        self.render_source_snippet(&mut out, source, use_color);
        self.render_hints(&mut out);

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

fn render_display_path(file: &str) -> Cow<'_, str> {
    let path = std::path::Path::new(file);
    if path.is_absolute()
        && let Ok(cwd) = std::env::current_dir()
        && let Ok(stripped) = path.strip_prefix(&cwd)
    {
        return Cow::Owned(stripped.to_string_lossy().to_string());
    }
    Cow::Borrowed(file)
}
