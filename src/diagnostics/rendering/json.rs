use serde::Serialize;

use crate::diagnostics::position::{Position, Span};
use crate::diagnostics::{
    Diagnostic, DiagnosticCategory, DiagnosticPhase, DiagnosticsAggregator, Hint, HintKind,
    InlineSuggestion, Label, LabelStyle, RelatedDiagnostic, RelatedKind, Severity,
};

#[derive(Serialize)]
struct JsonDiagnostic {
    severity: &'static str,
    category: Option<&'static str>,
    phase: Option<&'static str>,
    code: Option<String>,
    title: String,
    message: Option<String>,
    file: Option<String>,
    span: Option<JsonSpan>,
    labels: Vec<JsonLabel>,
    hints: Vec<JsonHint>,
    suggestions: Vec<JsonSuggestion>,
    related: Vec<JsonRelated>,
}

#[derive(Serialize)]
struct JsonPosition {
    line: usize,
    column: usize,
}

#[derive(Serialize)]
struct JsonSpan {
    start: JsonPosition,
    end: JsonPosition,
}

#[derive(Serialize)]
struct JsonLabel {
    style: &'static str,
    text: String,
    span: JsonSpan,
}

#[derive(Serialize)]
struct JsonHint {
    kind: &'static str,
    text: String,
    file: Option<String>,
    span: Option<JsonSpan>,
    label: Option<String>,
}

#[derive(Serialize)]
struct JsonSuggestion {
    replacement: String,
    message: Option<String>,
    span: JsonSpan,
}

#[derive(Serialize)]
struct JsonRelated {
    kind: &'static str,
    message: String,
    file: Option<String>,
    span: Option<JsonSpan>,
}

pub fn render_diagnostics_json(
    diagnostics: &[Diagnostic],
    default_file: Option<&str>,
    max_errors: Option<usize>,
    stage_filtering: bool,
    pretty: bool,
) -> String {
    let mut agg = DiagnosticsAggregator::new(diagnostics)
        .with_max_errors(max_errors)
        .with_stage_filtering(stage_filtering);
    if let Some(file) = default_file {
        agg = agg.with_default_file(file.to_string());
    }

    let processed = agg.processed_diagnostics();
    let payload: Vec<JsonDiagnostic> = processed.iter().map(JsonDiagnostic::from_diag).collect();
    if pretty {
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "[]".to_string())
    } else {
        serde_json::to_string(&payload).unwrap_or_else(|_| "[]".to_string())
    }
}

impl JsonDiagnostic {
    fn from_diag(diag: &Diagnostic) -> Self {
        Self {
            severity: severity_str(diag.severity()),
            category: diag.category().map(category_str),
            phase: diag.phase().map(phase_str),
            code: diag.code().map(ToString::to_string),
            title: diag.title().to_string(),
            message: diag.message().map(ToString::to_string),
            file: diag.file().map(ToString::to_string),
            span: diag.span().map(JsonSpan::from_span),
            labels: diag.labels().iter().map(JsonLabel::from_label).collect(),
            hints: diag.hints().iter().map(JsonHint::from_hint).collect(),
            suggestions: diag
                .suggestions()
                .iter()
                .map(JsonSuggestion::from_suggestion)
                .collect(),
            related: diag
                .related()
                .iter()
                .map(JsonRelated::from_related)
                .collect(),
        }
    }
}

fn category_str(category: DiagnosticCategory) -> &'static str {
    category.as_str()
}

fn phase_str(phase: DiagnosticPhase) -> &'static str {
    match phase {
        DiagnosticPhase::Parse => "parse",
        DiagnosticPhase::ModuleGraph => "module_graph",
        DiagnosticPhase::Validation => "validation",
        DiagnosticPhase::TypeInference => "type_inference",
        DiagnosticPhase::TypeCheck => "type_check",
        DiagnosticPhase::Effect => "effect",
        DiagnosticPhase::Runtime => "runtime",
    }
}

impl JsonPosition {
    fn from_position(pos: Position) -> Self {
        Self {
            line: pos.line,
            column: pos.column,
        }
    }
}

impl JsonSpan {
    fn from_span(span: Span) -> Self {
        Self {
            start: JsonPosition::from_position(span.start),
            end: JsonPosition::from_position(span.end),
        }
    }
}

impl JsonLabel {
    fn from_label(label: &Label) -> Self {
        Self {
            style: label_style_str(label.style),
            text: label.text.clone(),
            span: JsonSpan::from_span(label.span),
        }
    }
}

impl JsonHint {
    fn from_hint(hint: &Hint) -> Self {
        Self {
            kind: hint_kind_str(hint.kind),
            text: hint.text.clone(),
            file: hint.file.clone(),
            span: hint.span.map(JsonSpan::from_span),
            label: hint.label.clone(),
        }
    }
}

impl JsonSuggestion {
    fn from_suggestion(suggestion: &InlineSuggestion) -> Self {
        Self {
            replacement: suggestion.replacement.clone(),
            message: suggestion.message.clone(),
            span: JsonSpan::from_span(suggestion.span),
        }
    }
}

impl JsonRelated {
    fn from_related(related: &RelatedDiagnostic) -> Self {
        Self {
            kind: related_kind_str(related.kind),
            message: related.message.clone(),
            file: related.file.clone(),
            span: related.span.map(JsonSpan::from_span),
        }
    }
}

fn severity_str(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
        Severity::Help => "help",
    }
}

fn label_style_str(style: LabelStyle) -> &'static str {
    match style {
        LabelStyle::Primary => "primary",
        LabelStyle::Secondary => "secondary",
        LabelStyle::Note => "note",
    }
}

fn hint_kind_str(kind: HintKind) -> &'static str {
    match kind {
        HintKind::Hint => "hint",
        HintKind::Note => "note",
        HintKind::Help => "help",
        HintKind::Example => "example",
    }
}

fn related_kind_str(kind: RelatedKind) -> &'static str {
    match kind {
        RelatedKind::Note => "note",
        RelatedKind::Help => "help",
        RelatedKind::Related => "related",
    }
}
