use std::rc::Rc;

use crate::diagnostics::position::Span;
use crate::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticCategory, DiagnosticPhase, DiagnosticsAggregator,
    ErrorType, OCCURS_CHECK_FAILURE, RUNTIME_TYPE_ERROR, TYPE_UNIFICATION_ERROR, StackTraceFrame,
    diagnostic_for,
};

/// Labels used when presenting expected-versus-actual type notes.
#[derive(Debug, Clone, Copy)]
pub struct TypeMismatchNotes<'a> {
    pub expected_label: &'a str,
    pub actual_label: &'a str,
}

impl<'a> TypeMismatchNotes<'a> {
    /// Create a pair of note labels for expected and actual type lines.
    pub const fn new(expected_label: &'a str, actual_label: &'a str) -> Self {
        Self {
            expected_label,
            actual_label,
        }
    }
}

/// Return a targeted help hint for common expected-versus-actual type pairs.
pub fn type_pair_hint(expected: &str, actual: &str) -> Option<String> {
    match (expected, actual) {
        ("String", "Int") | ("String", "Float") => Some(format!(
            "Try `to_string(...)` to convert a `{actual}` into a `String`."
        )),
        ("Int", "Float") => {
            Some("Try `to_int(...)` if truncating a `Float` to an `Int` is intended.".to_string())
        }
        ("Float", "Int") => {
            Some("Try `to_float(...)` to widen this `Int` to a `Float`.".to_string())
        }
        ("Bool", "Int") | ("Bool", "Float") => {
            Some("Booleans are not numeric in Flux. Use `true` or `false` here.".to_string())
        }
        _ if expected.starts_with("Option<") && !actual.starts_with("Option<") => {
            Some("Wrap this value in `Some(...)` or return `None`.".to_string())
        }
        _ if actual.starts_with("Option<") && !expected.starts_with("Option<") => Some(
            "This value might be `None`. Use `match` to unwrap it before using it here."
                .to_string(),
        ),
        _ => None,
    }
}

// Diagnostic tone guide:
// - display_title: short noun phrase naming the real problem
// - message: human-first explanation of what likely went wrong
// - labels: what this code looks like to the compiler
// - help: one short concrete next step

/// Map a parser-facing display title to the category used for rendering and filtering.
pub fn parser_category_for_display_title(display_title: &str) -> DiagnosticCategory {
    match display_title {
        "Missing Function Body"
        | "Missing Module Body"
        | "Missing If Body"
        | "Missing Else Body"
        | "Missing Do Block"
        | "Missing Match Body"
        | "Missing Function Parameter List"
        | "Missing Import Path"
        | "Missing Import Alias"
        | "Missing Import Except List"
        | "Invalid Import Except List"
        | "Missing Effect Body"
        | "Missing Effect Name"
        | "Missing Data Type Name"
        | "Missing Data Body"
        | "Invalid Data Constructor"
        | "Missing Type Name"
        | "Missing Type Definition"
        | "Invalid Type Variant" => DiagnosticCategory::ParserDeclaration,
        "Missing Closing Delimiter" | "Unexpected Closing Delimiter" => {
            DiagnosticCategory::ParserDelimiter
        }
        "Missing Match Arm Arrow"
        | "Missing Lambda Arrow"
        | "Missing Hash Colon"
        | "Missing Effect Operation Colon"
        | "Missing Constructor Field Separator"
        | "Missing Handle Arm Arrow"
        | "Missing Parameter Separator"
        | "Invalid Match Arm Separator"
        | "Missing Effect Operation Separator"
        | "Missing Effect Operation Name" => DiagnosticCategory::ParserSeparator,
        "Invalid Handle Arm" | "Missing Parameter Name" | "Invalid Effect Operation" => {
            DiagnosticCategory::ParserExpression
        }
        _ => DiagnosticCategory::ParserExpression,
    }
}

/// Build a parser diagnostic for constructs that are missing their opening token.
pub fn missing_construct_opener_diagnostic(
    code: &'static crate::diagnostics::types::ErrorCode,
    span: Span,
    display_title: &str,
    category: DiagnosticCategory,
    message: impl Into<String>,
    primary_label: impl Into<String>,
    help: impl Into<String>,
) -> Diagnostic {
    diagnostic_for(code)
        .with_display_title(display_title)
        .with_category(category)
        .with_span(span)
        .with_message(message.into())
        .with_primary_label(span, primary_label.into())
        .with_help(help.into())
}

/// Build a parser diagnostic for a missing syntax token without an origin label.
pub fn missing_syntax_token_diagnostic(
    code: &'static crate::diagnostics::types::ErrorCode,
    span: Span,
    display_title: &str,
    category: DiagnosticCategory,
    message: impl Into<String>,
    help: impl Into<String>,
) -> Diagnostic {
    diagnostic_for(code)
        .with_display_title(display_title)
        .with_category(category)
        .with_span(span)
        .with_message(message.into())
        .with_help(help.into())
}

/// Build a parser diagnostic for a missing syntax token and attach its origin label.
pub fn missing_syntax_token_diagnostic_with_origin(
    code: &'static crate::diagnostics::types::ErrorCode,
    span: Span,
    display_title: &str,
    category: DiagnosticCategory,
    message: impl Into<String>,
    origin_label: impl Into<String>,
    help: impl Into<String>,
) -> Diagnostic {
    diagnostic_for(code)
        .with_display_title(display_title)
        .with_category(category)
        .with_span(span)
        .with_message(message.into())
        .with_primary_label(span, origin_label.into())
        .with_help(help.into())
}

/// Build a type mismatch diagnostic with expected/actual notes and a best-effort help hint.
pub fn type_mismatch_diagnostic(
    file: impl Into<Rc<str>>,
    span: Span,
    message: impl Into<String>,
    primary_label: impl Into<String>,
    expected: &str,
    actual: &str,
    notes: TypeMismatchNotes<'_>,
    fallback_hint: impl Into<String>,
) -> Diagnostic {
    diagnostic_for(&TYPE_UNIFICATION_ERROR)
        .with_category(DiagnosticCategory::TypeInference)
        .with_phase(DiagnosticPhase::TypeInference)
        .with_file(file)
        .with_span(span)
        .with_message(message.into())
        .with_primary_label(span, primary_label.into())
        .with_note(format!("{}: {}", notes.expected_label, expected))
        .with_note(format!("{}: {}", notes.actual_label, actual))
        .with_help(type_pair_hint(expected, actual).unwrap_or_else(|| fallback_hint.into()))
}

/// Build an occurs-check diagnostic for an inferred infinite type.
pub fn occurs_check_diagnostic(file: impl Into<Rc<str>>, span: Span, ty: &str) -> Diagnostic {
    diagnostic_for(&OCCURS_CHECK_FAILURE)
        .with_display_title("Infinite Type")
        .with_category(DiagnosticCategory::TypeInference)
        .with_phase(DiagnosticPhase::TypeInference)
        .with_file(file)
        .with_span(span)
        .with_message("I found a type that would be infinitely recursive.")
        .with_primary_label(span, format!("this expression would have the infinite type `{ty}`"))
        .with_help(
            "A value cannot contain itself directly. If you need recursive data, define an ADT wrapper first.",
        )
}

/// Truncate and normalize a runtime value preview for note output.
pub fn runtime_value_preview(value: &str) -> String {
    const LIMIT: usize = 48;
    let mut preview = value.trim().replace('\n', "\\n");
    if preview.len() > LIMIT {
        preview.truncate(LIMIT);
        preview.push_str("...");
    }
    preview
}

/// Build a runtime type error diagnostic with optional value preview context.
pub fn runtime_type_error_diagnostic(
    file: impl Into<Rc<str>>,
    span: Span,
    expected: &str,
    actual: &str,
    value_preview: Option<&str>,
) -> Diagnostic {
    let mut diag = diagnostic_for(&RUNTIME_TYPE_ERROR)
        .with_display_title("Type Error")
        .with_category(DiagnosticCategory::RuntimeType)
        .with_phase(DiagnosticPhase::Runtime)
        .with_file(file)
        .with_span(span)
        .with_message("I found a value with the wrong runtime type.")
        .with_primary_label(span, format!("this value has runtime type `{actual}`"))
        .with_note(format!("expected type: {expected}"))
        .with_note(format!("found type:    {actual}"));

    if let Some(value_preview) = value_preview {
        diag = diag.with_note(format!(
            "runtime value:  {}",
            runtime_value_preview(value_preview)
        ));
    }

    diag.with_help(
        "Check the value flowing into this operation or add a conversion before this point.",
    )
}

/// Render a runtime diagnostic with optional source text and appended stack frames.
pub fn render_runtime_diagnostic(
    diag: &Diagnostic,
    source_file: &str,
    source_text: Option<&str>,
    stack_frames: &[String],
) -> String {
    let diag = if stack_frames.is_empty() {
        diag.clone()
    } else {
        diag.clone().with_stack_trace(
            stack_frames
                .iter()
                .cloned()
                .map(StackTraceFrame::new)
                .collect::<Vec<_>>(),
        )
    };

    let mut agg =
        DiagnosticsAggregator::new(std::slice::from_ref(&diag)).with_file_headers(false);
    if let Some(src) = source_text {
        agg = agg.with_source(source_file.to_string(), src.to_string());
    }

    agg.report().rendered
}

/// Attach a short explanation that a runtime diagnostic came from a dynamic boundary.
pub fn dynamic_explained_diagnostic(
    code: &str,
    title: &str,
    message: impl Into<String>,
    file: impl Into<Rc<str>>,
    span: Span,
    primary_label: impl Into<String>,
    notes: impl IntoIterator<Item = String>,
    help: impl Into<String>,
) -> Diagnostic {
    let mut diag = Diagnostic::make_error_dynamic(
        code,
        title,
        ErrorType::Compiler,
        message.into(),
        None,
        file,
        span,
    )
    .with_primary_label(span, primary_label.into());

    for note in notes {
        diag = diag.with_note(note);
    }

    diag.with_help(help.into())
}

/// Build a note explaining that a module was skipped after earlier failures.
pub fn module_skipped_note(
    file: impl Into<Rc<str>>,
    skipped_module: impl Into<String>,
    dependency_name: impl Into<String>,
) -> Diagnostic {
    Diagnostic::make_note(
        "MODULE SKIPPED",
        format!(
            "I skipped module `{}` because its dependency `{}` already has errors.",
            skipped_module.into(),
            dependency_name.into()
        ),
        file,
        Span::default(),
    )
    .with_display_title("Module Skipped")
    .with_category(DiagnosticCategory::Orchestration)
    .with_phase(DiagnosticPhase::Validation)
}

/// Build a note summarizing diagnostics suppressed by stage filtering.
pub fn downstream_errors_suppressed_note(
    file: impl Into<Rc<str>>,
    suppressed_type_count: usize,
    suppressed_effect_count: usize,
) -> Diagnostic {
    let suppressed_total = suppressed_type_count + suppressed_effect_count;
    let mut details = Vec::new();
    if suppressed_type_count > 0 {
        details.push(format!("{} type", suppressed_type_count));
    }
    if suppressed_effect_count > 0 {
        details.push(format!("{} effect", suppressed_effect_count));
    }
    let breakdown = if details.is_empty() {
        "later-stage".to_string()
    } else {
        details.join(", ")
    };

    Diagnostic::make_note(
        "DOWNSTREAM ERRORS SUPPRESSED",
        format!(
            "I hid {} later-stage diagnostic{} ({}) because earlier errors make them less reliable. Fix the earlier errors first, or use `--all-errors` to see everything.",
            suppressed_total,
            if suppressed_total == 1 { "" } else { "s" },
            breakdown,
        ),
        file,
        Span::default(),
    )
    .with_display_title("Downstream Errors Suppressed")
    .with_category(DiagnosticCategory::Orchestration)
    .with_phase(DiagnosticPhase::Validation)
}
