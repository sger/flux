use crate::frontend::position::{Position, Span};
use super::{ErrorCode, ErrorType, format_message};
use std::borrow::Cow;
use std::env;

// Error code constants for special rendering cases
const UNTERMINATED_STRING_ERROR_CODE: &str = "E031";
// Sentinel value for end-of-line positions.
const END_OF_LINE_SENTINEL: usize = usize::MAX - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Help,
}

/// Kind of hint to display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HintKind {
    /// General hint or suggestion (default)
    Hint,
    /// Additional context or information
    Note,
    /// Explicit help on how to fix
    Help,
    /// Code example demonstrating the solution
    Example,
}

/// A hint with optional source location and label
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub kind: HintKind,
    pub text: String,
    pub span: Option<Span>,
    pub label: Option<String>,
    pub file: Option<String>,
}

impl Hint {
    /// Create a simple text-only hint
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: HintKind::Hint,
            text: text.into(),
            span: None,
            label: None,
            file: None,
        }
    }

    /// Create a hint with a source location
    pub fn at(text: impl Into<String>, span: Span) -> Self {
        Self {
            kind: HintKind::Hint,
            text: text.into(),
            span: Some(span),
            label: None,
            file: None,
        }
    }

    /// Create a hint with a source location and label
    pub fn labeled(text: impl Into<String>, span: Span, label: impl Into<String>) -> Self {
        Self {
            kind: HintKind::Hint,
            text: text.into(),
            span: Some(span),
            label: Some(label.into()),
            file: None,
        }
    }

    /// Create a note hint
    pub fn note(text: impl Into<String>) -> Self {
        Self {
            kind: HintKind::Note,
            text: text.into(),
            span: None,
            label: None,
            file: None,
        }
    }

    /// Create a help hint
    pub fn help(text: impl Into<String>) -> Self {
        Self {
            kind: HintKind::Help,
            text: text.into(),
            span: None,
            label: None,
            file: None,
        }
    }

    /// Create an example hint
    pub fn example(text: impl Into<String>) -> Self {
        Self {
            kind: HintKind::Example,
            text: text.into(),
            span: None,
            label: None,
            file: None,
        }
    }

    /// Add a label to this hint
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the file for this hint (for cross-file references)
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
}

/// Style for inline source labels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabelStyle {
    /// Primary label - main focus of the error (rendered in red)
    Primary,
    /// Secondary label - additional context (rendered in blue)
    Secondary,
    /// Note label - informational (rendered in cyan)
    Note,
}

/// An inline label that annotates a specific span within the source snippet
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub text: String,
    pub style: LabelStyle,
}

impl Label {
    /// Create a primary label (main error location)
    pub fn primary(span: Span, text: impl Into<String>) -> Self {
        Self {
            span,
            text: text.into(),
            style: LabelStyle::Primary,
        }
    }

    /// Create a secondary label (additional context)
    pub fn secondary(span: Span, text: impl Into<String>) -> Self {
        Self {
            span,
            text: text.into(),
            style: LabelStyle::Secondary,
        }
    }

    /// Create a note label (informational)
    pub fn note(span: Span, text: impl Into<String>) -> Self {
        Self {
            span,
            text: text.into(),
            style: LabelStyle::Note,
        }
    }
}

/// An inline suggestion that shows how to fix the code
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineSuggestion {
    pub replacement: String,
    pub span: Span,
    pub message: Option<String>,
}

impl InlineSuggestion {
    /// Create a suggestion with a replacement text
    pub fn new(span: Span, replacement: impl Into<String>) -> Self {
        Self {
            span,
            replacement: replacement.into(),
            message: None,
        }
    }

    /// Add a message explaining the suggestion
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

/// A hint chain that provides step-by-step guidance for complex errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HintChain {
    pub steps: Vec<String>,
    pub conclusion: Option<String>,
}

impl HintChain {
    /// Create a new hint chain with the given steps
    pub fn new(steps: Vec<String>) -> Self {
        Self {
            steps,
            conclusion: None,
        }
    }

    /// Add a conclusion to the hint chain
    pub fn with_conclusion(mut self, conclusion: impl Into<String>) -> Self {
        self.conclusion = Some(conclusion.into());
        self
    }

    /// Create a hint chain from a slice of step strings
    pub fn from_steps<S: Into<String>>(steps: impl IntoIterator<Item = S>) -> Self {
        Self {
            steps: steps.into_iter().map(|s| s.into()).collect(),
            conclusion: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelatedKind {
    Note,
    Help,
    Related,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedDiagnostic {
    pub kind: RelatedKind,
    pub message: String,
    pub span: Option<Span>,
    pub file: Option<String>,
}

impl RelatedDiagnostic {
    pub fn note(message: impl Into<String>) -> Self {
        Self {
            kind: RelatedKind::Note,
            message: message.into(),
            span: None,
            file: None,
        }
    }

    pub fn help(message: impl Into<String>) -> Self {
        Self {
            kind: RelatedKind::Help,
            message: message.into(),
            span: None,
            file: None,
        }
    }

    pub fn related(message: impl Into<String>) -> Self {
        Self {
            kind: RelatedKind::Related,
            message: message.into(),
            span: None,
            file: None,
        }
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
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
    labels: Vec<Label>,
    hints: Vec<Hint>,
    suggestions: Vec<InlineSuggestion>,
    hint_chains: Vec<HintChain>,
    related: Vec<RelatedDiagnostic>,
}

// ICE = Internal Compiler Error (a compiler bug, not user code).
#[macro_export]
macro_rules! ice {
    ($msg:expr) => {
        $crate::frontend::diagnostics::Diagnostic::error("INTERNAL COMPILER ERROR")
            .with_message($msg)
            .with_hint_text(format!("{}:{} ({})", file!(), line!(), module_path!()))
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
            labels: Vec::new(),
            hints: Vec::new(),
            suggestions: Vec::new(),
            hint_chains: Vec::new(),
            related: Vec::new(),
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
            labels: Vec::new(),
            hints: Vec::new(),
            suggestions: Vec::new(),
            hint_chains: Vec::new(),
            related: Vec::new(),
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

    pub fn hints(&self) -> &[Hint] {
        &self.hints
    }

    pub fn labels(&self) -> &[Label] {
        &self.labels
    }

    pub fn suggestions(&self) -> &[InlineSuggestion] {
        &self.suggestions
    }

    pub fn hint_chains(&self) -> &[HintChain] {
        &self.hint_chains
    }

    pub fn related(&self) -> &[RelatedDiagnostic] {
        &self.related
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

    /// Add a hint to the diagnostic
    pub fn with_hint(mut self, hint: Hint) -> Self {
        self.hints.push(hint);
        self
    }

    /// Add a text-only hint (convenience method for backward compatibility)
    pub fn with_hint_text(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        let cleaned = text
            .strip_prefix("Hint:\n")
            .or_else(|| text.strip_prefix("Hint:"))
            .unwrap_or(text.as_str())
            .trim_start();
        self.hints.push(Hint::text(cleaned));
        self
    }

    /// Add a hint with a source location (convenience method)
    pub fn with_hint_at(mut self, text: impl Into<String>, span: Span) -> Self {
        self.hints.push(Hint::at(text, span));
        self
    }

    /// Add a hint with a source location and label (convenience method)
    pub fn with_hint_labeled(
        mut self,
        text: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) -> Self {
        self.hints.push(Hint::labeled(text, span, label));
        self
    }

    /// Add a note hint (additional context or information)
    pub fn with_note(mut self, text: impl Into<String>) -> Self {
        self.hints.push(Hint::note(text));
        self
    }

    /// Add a help hint (explicit instructions on how to fix)
    pub fn with_help(mut self, text: impl Into<String>) -> Self {
        self.hints.push(Hint::help(text));
        self
    }

    /// Add an example hint (code example demonstrating the solution)
    pub fn with_example(mut self, text: impl Into<String>) -> Self {
        self.hints.push(Hint::example(text));
        self
    }

    /// Add a primary label to the diagnostic (main error location)
    pub fn with_primary_label(mut self, span: Span, text: impl Into<String>) -> Self {
        self.labels.push(Label::primary(span, text));
        self
    }

    /// Add a secondary label to the diagnostic (additional context)
    pub fn with_secondary_label(mut self, span: Span, text: impl Into<String>) -> Self {
        self.labels.push(Label::secondary(span, text));
        self
    }

    /// Add a note label to the diagnostic (informational)
    pub fn with_note_label(mut self, span: Span, text: impl Into<String>) -> Self {
        self.labels.push(Label::note(span, text));
        self
    }

    /// Add a label with explicit style
    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    /// Add an inline code suggestion
    pub fn with_suggestion(mut self, suggestion: InlineSuggestion) -> Self {
        self.suggestions.push(suggestion);
        self
    }

    /// Add an inline suggestion with replacement text (convenience method)
    pub fn with_suggestion_replace(
        mut self,
        span: Span,
        replacement: impl Into<String>,
    ) -> Self {
        self.suggestions.push(InlineSuggestion::new(span, replacement));
        self
    }

    /// Add an inline suggestion with message (convenience method)
    pub fn with_suggestion_message(
        mut self,
        span: Span,
        replacement: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        self.suggestions.push(
            InlineSuggestion::new(span, replacement)
                .with_message(message)
        );
        self
    }

    /// Add a hint chain for step-by-step guidance
    pub fn with_hint_chain(mut self, chain: HintChain) -> Self {
        self.hint_chains.push(chain);
        self
    }

    /// Add a related diagnostic entry (note/help/related)
    pub fn with_related(mut self, related: RelatedDiagnostic) -> Self {
        self.related.push(related);
        self
    }

    /// Add a hint chain from a list of steps (convenience method)
    pub fn with_steps<S: Into<String>>(
        mut self,
        steps: impl IntoIterator<Item = S>,
    ) -> Self {
        self.hint_chains.push(HintChain::from_steps(steps));
        self
    }

    /// Add a hint chain with steps and conclusion (convenience method)
    pub fn with_steps_and_conclusion<S: Into<String>>(
        mut self,
        steps: impl IntoIterator<Item = S>,
        conclusion: impl Into<String>,
    ) -> Self {
        self.hint_chains.push(
            HintChain::from_steps(steps).with_conclusion(conclusion)
        );
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
            diag = diag.with_hint_text(hint_text);
        }

        diag
    }

    /// Generic warning builder using ErrorCode specification
    /// Similar to make_error but creates warnings for non-fatal issues
    pub fn make_warning_from_code(
        warn_spec: &'static ErrorCode,
        values: &[&str],
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        let message = format_message(warn_spec.message, values);
        let hint = warn_spec.hint.map(|h| format_message(h, values));

        let mut diag = Diagnostic::warning(warn_spec.title)
            .with_code(warn_spec.code)
            .with_error_type(warn_spec.error_type)
            .with_file(file)
            .with_span(span)
            .with_message(message);

        if let Some(hint_text) = hint {
            diag = diag.with_hint_text(hint_text);
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
            diag = diag.with_hint_text(hint_text);
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
            labels: Vec::new(),
            hints: Vec::new(),
            suggestions: Vec::new(),
            hint_chains: Vec::new(),
            related: Vec::new(),
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
            labels: Vec::new(),
            hints: Vec::new(),
            suggestions: Vec::new(),
            hint_chains: Vec::new(),
            related: Vec::new(),
        }
    }

    fn render_header(
        &self,
        out: &mut String,
        use_color: bool,
        code: &str,
    ) {
        let yellow = "\u{1b}[33m";
        let reset = "\u{1b}[0m";
        // Header: -- Compiler error: expected expression [E031]
        if use_color {
            out.push_str(yellow);
        }
        out.push_str(&format!(
            "-- {}: {} [{}]",
            self.header_label(),
            self.title,
            code
        ));
        if use_color {
            out.push_str(reset);
        }
        out.push('\n');
    }

    fn render_message(&self, out: &mut String) {
        // Message
        if let Some(message) = &self.message {
            if !message.is_empty() {
                out.push('\n');
                out.push_str(message);
                out.push('\n');
                return;
            }
        }
        // Keep a blank line between header and location when no message is provided.
        out.push('\n');
    }

    fn render_location(
        &self,
        out: &mut String,
        source: Option<&str>,
        file: &str,
    ) {
        // Location indicator: --> file:line:column
        if let Some(position) = self.position() {
            // Only add newline if there was a non-empty message
            if self.message.as_ref().map_or(false, |m| !m.is_empty()) {
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
        let blue = "\u{1b}[34m";
        let cyan = "\u{1b}[36m";
        let reset = "\u{1b}[0m";

        if let Some(position) = self.position() {
            let span = self.span.unwrap_or_else(|| Span::new(position, position));
            let start_line = span.start.line;
            let end_line = span.end.line.max(start_line);

            // Expand range to include all label lines
            let label_start = self.labels.iter().map(|l| l.span.start.line).min().unwrap_or(start_line);
            let label_end = self.labels.iter().map(|l| l.span.end.line).max().unwrap_or(end_line);
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

                    // Render labels for this line
                    for label in &self.labels {
                        // Only render labels that are on this specific line
                        if label.span.start.line == line_no && label.span.end.line == line_no {
                            let label_start = label.span.start.column.min(line_len);
                            let label_end = label.span.end.column.min(line_len).max(label_start + 1);
                            let label_len = label_end.saturating_sub(label_start);

                            // Choose color based on label style
                            let color = match label.style {
                                LabelStyle::Primary => red,
                                LabelStyle::Secondary => blue,
                                LabelStyle::Note => cyan,
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
                                out.push_str(reset);
                            }
                        }
                    }
                }
            }
        }
    }

    fn render_suggestions(&self, out: &mut String, source: Option<&str>, use_color: bool) {
        if self.suggestions.is_empty() {
            return;
        }

        let green = "\u{1b}[32m";
        let reset = "\u{1b}[0m";

        for suggestion in &self.suggestions {
            let span = suggestion.span;
            let line_no = span.start.line;

            // Get the source line
            if let Some(line_text) = source.and_then(|src| get_source_line(src, line_no)) {
                let line_width = line_no.to_string().len();

                // Show "help:" message
                out.push_str("   |\n");
                if use_color {
                    out.push_str(green);
                }
                if let Some(msg) = &suggestion.message {
                    out.push_str(&format!("help: {}\n", msg));
                } else {
                    out.push_str(&format!("help: Replace with '{}'\n", suggestion.replacement));
                }
                if use_color {
                    out.push_str(reset);
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

                out.push_str(&format!("   |\n"));
                out.push_str(&format!(
                    "{:>width$} | {}\n",
                    line_no, replaced_line, width = line_width
                ));

                // Show tildes under the replacement
                out.push_str(&format!(
                    "{:>width$} | {}",
                    "", " ".repeat(start_col), width = line_width
                ));
                if use_color {
                    out.push_str(green);
                }
                out.push_str(&"~".repeat(suggestion.replacement.len()));
                if use_color {
                    out.push_str(reset);
                }
                out.push('\n');
            }
        }
    }

    fn render_hints(&self, out: &mut String, source: Option<&str>, use_color: bool) {
        if self.hints.is_empty() && self.hint_chains.is_empty() {
            return;
        }

        let blue = "\u{1b}[34m";
        let cyan = "\u{1b}[36m";
        let green = "\u{1b}[32m";
        let reset = "\u{1b}[0m";

        // Separate hints into those with and without spans
        let (text_hints, span_hints): (Vec<_>, Vec<_>) =
            self.hints.iter().partition(|h| h.span.is_none());

        let has_text_hints = !text_hints.is_empty();

        // Group text-only hints by kind
        let mut hints_by_kind: std::collections::HashMap<HintKind, Vec<&Hint>> =
            std::collections::HashMap::new();
        for hint in text_hints {
            hints_by_kind.entry(hint.kind).or_default().push(hint);
        }

        // Render text-only hints grouped by kind
        // Order: Hint, Note, Help, Example
        for kind in [HintKind::Hint, HintKind::Note, HintKind::Help, HintKind::Example] {
            if let Some(hints) = hints_by_kind.get(&kind) {
                out.push_str("\n\n");
                let (label, color) = match kind {
                    HintKind::Hint => ("Hint", blue),
                    HintKind::Note => ("Note", cyan),
                    HintKind::Help => ("Help", green),
                    HintKind::Example => ("Example", blue),
                };
                if use_color {
                    out.push_str(color);
                }
                out.push_str(&format!("{}:\n", label));
                if use_color {
                    out.push_str(reset);
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
                // Use single newline if text hints already added double newline
                if has_text_hints {
                    out.push('\n');
                } else {
                    out.push_str("\n\n");
                }

                // Render the note header with optional label
                if let Some(label) = &hint.label {
                    if use_color {
                        out.push_str(blue);
                    }
                    out.push_str(&format!("   = note: {}\n", label));
                    if use_color {
                        out.push_str(reset);
                    }
                } else {
                    if use_color {
                        out.push_str(blue);
                    }
                    out.push_str("   = note:\n");
                    if use_color {
                        out.push_str(reset);
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
                    .or_else(|| self.file.as_deref())
                    .filter(|f| !f.is_empty())
                    .map(render_display_path)
                    .unwrap_or_else(|| Cow::Borrowed("<unknown>"));
                out.push_str(&format!(
                    "  --> {}:{}:{}\n",
                    file, start.line, display_col
                ));

                // Render source snippet for this hint
                self.render_hint_snippet(out, source, span, use_color);

                // Render hint text if provided
                if !hint.text.is_empty() {
                    let (label, color) = match hint.kind {
                        HintKind::Hint => ("Hint", blue),
                        HintKind::Note => ("Note", cyan),
                        HintKind::Help => ("Help", green),
                        HintKind::Example => ("Example", blue),
                    };
                    out.push_str("\n\n");
                    if use_color {
                        out.push_str(color);
                    }
                    out.push_str(&format!("{}:\n", label));
                    if use_color {
                        out.push_str(reset);
                    }
                    out.push_str(&format!("  {}\n", hint.text));
                }
            }
        }

        // Render hint chains
        for chain in &self.hint_chains {
            out.push_str("\n\n");
            if use_color {
                out.push_str(blue);
            }
            out.push_str("Hint:\n");
            if use_color {
                out.push_str(reset);
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

    fn render_hint_snippet(&self, out: &mut String, source: Option<&str>, span: Span, use_color: bool) {
        let red = "\u{1b}[31m";
        let reset = "\u{1b}[0m";

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
                    line_no, line_text, width = line_width
                ));
                out.push_str(&format!(
                    "{:>width$} | {}",
                    "", " ".repeat(caret_start), width = line_width
                ));
                if use_color {
                    out.push_str(red);
                }
                let caret_len = caret_end.saturating_sub(caret_start).max(1);
                out.push_str(&"^".repeat(caret_len));
                if use_color {
                    out.push_str(reset);
                }

                if line_no < end_line {
                    out.push('\n');
                }
            }
        }
    }

    fn render_related(
        &self,
        out: &mut String,
        source: Option<&str>,
        default_file: Option<&str>,
        use_color: bool,
    ) {
        if self.related.is_empty() {
            return;
        }

        let blue = "\u{1b}[34m";
        let cyan = "\u{1b}[36m";
        let green = "\u{1b}[32m";
        let reset = "\u{1b}[0m";

        for related in &self.related {
            out.push_str("\n\n");
            let (label, color) = match related.kind {
                RelatedKind::Note => ("note", cyan),
                RelatedKind::Help => ("help", green),
                RelatedKind::Related => ("related", blue),
            };
            if use_color {
                out.push_str(color);
            }
            if related.message.is_empty() {
                out.push_str(&format!("{}:", label));
            } else {
                out.push_str(&format!("{}: {}", label, related.message));
            }
            if use_color {
                out.push_str(reset);
            }
            out.push('\n');

            if let Some(span) = related.span {
                let related_source = match related.file.as_deref() {
                    Some(file) => {
                        if self.file.as_deref() == Some(file) || default_file == Some(file) {
                            source
                        } else {
                            None
                        }
                    }
                    None => source,
                };
                let file = related
                    .file
                    .as_deref()
                    .or_else(|| self.file.as_deref())
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
                out.push_str(&format!(
                    "  --> {}:{}:{}\n",
                    file, start.line, display_col
                ));
                self.render_hint_snippet(out, related_source, span, use_color);
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

        self.render_header(&mut out, use_color, code);
        self.render_message(&mut out);
        self.render_location(&mut out, source, file.as_ref());
        self.render_source_snippet(&mut out, source, use_color);
        self.render_suggestions(&mut out, source, use_color);
        self.render_hints(&mut out, source, use_color);
        self.render_related(&mut out, source, default_file, use_color);

        if !out.ends_with('\n') {
            out.push('\n');
        }

        out
    }
}

impl Diagnostic {
    fn header_label(&self) -> &'static str {
        match self.severity {
            Severity::Error => self
                .error_type
                .map(|error_type| error_type.prefix())
                .unwrap_or("Error"),
            Severity::Warning => "Warning",
            Severity::Note => "Note",
            Severity::Help => "Help",
        }
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

pub fn render_display_path(file: &str) -> Cow<'_, str> {
    let path = std::path::Path::new(file);
    if path.is_absolute()
        && let Ok(cwd) = std::env::current_dir()
        && let Ok(stripped) = path.strip_prefix(&cwd)
    {
        return Cow::Owned(stripped.to_string_lossy().to_string());
    }
    Cow::Borrowed(file)
}
