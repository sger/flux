use std::path::PathBuf;

use crate::syntax::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticsAggregator, ErrorType, NOT_A_FUNCTION, Severity,
    diag_enhanced, format_message, format_message_named, lookup_error_code,
};
use crate::syntax::position::{Position, Span};

#[test]
fn format_message_replaces_placeholders() {
    let msg = format_message("Expected {}, got {}.", &["Int", "String"]);
    assert_eq!(msg, "Expected Int, got String.");
}

#[test]
fn format_message_named_replaces_named_placeholders() {
    let msg = format_message_named(
        "Cannot access {member} in {module}.",
        &[("member", "foo"), ("module", "Bar")],
    );
    assert_eq!(msg, "Cannot access foo in Bar.");
}

#[test]
fn lookup_error_code_and_diag_enhanced() {
    let code = lookup_error_code("E1001").expect("E1001 exists");
    assert_eq!(code.code, NOT_A_FUNCTION.code);

    let diag = diag_enhanced(&NOT_A_FUNCTION);
    assert_eq!(diag.code(), Some("E1001"));
    assert_eq!(diag.error_type(), Some(ErrorType::Runtime));
}

#[test]
fn diagnostic_position_uses_span_start() {
    let span = Span::new(Position::new(2, 4), Position::new(2, 8));
    let diag = Diagnostic::warning("TEST").with_span(span);

    assert_eq!(diag.position(), Some(Position::new(2, 4)));
}

#[test]
fn aggregator_deduplicates_identical_diagnostics() {
    let diag = Diagnostic::warning("OOPS")
        .with_message("boom")
        .with_error_type(ErrorType::Compiler);
    let diagnostics = vec![diag.clone(), diag];

    let report = DiagnosticsAggregator::new(&diagnostics).report();
    assert_eq!(report.counts.warnings, 1);
    assert_eq!(report.counts.total(), 1);
}

#[test]
fn render_display_path_strips_cwd_prefix() {
    let cwd = std::env::current_dir().unwrap();
    let mut path = PathBuf::from(&cwd);
    path.push("src");
    path.push("main.rs");

    let path_str = path.to_string_lossy();
    let display = crate::syntax::diagnostics::render_display_path(path_str.as_ref());

    assert_eq!(display.as_ref(), "src/main.rs");
}

#[test]
fn severity_ordering_is_stable() {
    let error = diag_enhanced(&NOT_A_FUNCTION);
    let warning = Diagnostic::warning("WARN");

    assert_eq!(error.severity(), Severity::Error);
    assert_eq!(warning.severity(), Severity::Warning);
}
