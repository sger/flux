use flux::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticPhase, ErrorType,
    position::{Position, Span},
    render_diagnostics_json,
};

mod diagnostics_env;

#[test]
fn render_uses_span_for_caret() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let source = "let x = 1;\nlet y = x + 2;\n";
    let span = Span::new(Position::new(2, 8), Position::new(2, 9)); // points at 'x'
    let diag = Diagnostic::warning("TEST")
        .with_code("E999")
        .with_file("test.flx")
        .with_span(span)
        .with_message("span test");

    let out = diag.render(Some(source), Some("test.flx"));

    assert!(out.contains("  --> test.flx:2:9"));
    assert!(out.contains("2 | let y = x + 2;"));
    let lines: Vec<&str> = out.lines().collect();
    let snippet_idx = lines
        .iter()
        .position(|line| line.contains("2 | let y = x + 2;"))
        .expect("missing snippet line");
    let caret_line = lines.get(snippet_idx + 1).expect("missing caret line");
    let caret_col = caret_line.find('^').expect("missing caret");
    let snippet_line = lines[snippet_idx];
    let x_col = snippet_line.find("x + 2;").expect("missing x in snippet");
    assert_eq!(caret_col, x_col);
}

#[test]
fn json_render_is_valid_array() {
    let diag = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "type mismatch",
        None,
        "test.flx",
        Span::new(Position::new(1, 1), Position::new(1, 2)),
    );
    let out = render_diagnostics_json(&[diag], Some("test.flx"), Some(50), true);
    let parsed: serde_json::Value =
        serde_json::from_str(&out).expect("expected valid JSON diagnostics output");
    assert!(parsed.is_array(), "expected top-level JSON array");
}

#[test]
fn json_render_contains_required_fields() {
    let diag = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "type mismatch",
        None,
        "test.flx",
        Span::new(Position::new(1, 1), Position::new(1, 2)),
    );
    let out = render_diagnostics_json(&[diag], Some("test.flx"), Some(50), true);
    let parsed: serde_json::Value =
        serde_json::from_str(&out).expect("expected valid JSON diagnostics output");
    let first = parsed
        .as_array()
        .and_then(|arr| arr.first())
        .expect("expected at least one diagnostic");
    for key in [
        "severity",
        "phase",
        "code",
        "title",
        "message",
        "file",
        "span",
        "labels",
        "hints",
        "suggestions",
        "related",
    ] {
        assert!(first.get(key).is_some(), "missing required field `{key}`");
    }
}

#[test]
fn json_render_includes_phase_when_tagged() {
    let diag = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "type mismatch",
        None,
        "test.flx",
        Span::new(Position::new(1, 1), Position::new(1, 2)),
    )
    .with_phase(DiagnosticPhase::TypeInference);
    let out = render_diagnostics_json(&[diag], Some("test.flx"), Some(50), true);
    let parsed: serde_json::Value =
        serde_json::from_str(&out).expect("expected valid JSON diagnostics output");
    let first = parsed
        .as_array()
        .and_then(|arr| arr.first())
        .expect("expected at least one diagnostic");
    assert_eq!(
        first.get("phase").and_then(|v| v.as_str()),
        Some("type_inference")
    );
}

#[test]
fn json_render_preserves_label_and_hint_payloads() {
    let span = Span::new(Position::new(2, 4), Position::new(2, 8));
    let diag = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "type mismatch",
        None,
        "test.flx",
        span,
    )
    .with_primary_label(span, "primary label")
    .with_secondary_label(span, "secondary label")
    .with_hint_text("example hint");

    let out = render_diagnostics_json(&[diag], Some("test.flx"), Some(50), true);
    let parsed: serde_json::Value =
        serde_json::from_str(&out).expect("expected valid JSON diagnostics output");
    let first = parsed
        .as_array()
        .and_then(|arr| arr.first())
        .expect("expected at least one diagnostic");
    let labels = first
        .get("labels")
        .and_then(|v| v.as_array())
        .expect("labels should be an array");
    assert!(labels.iter().any(|l| {
        l.get("text")
            .and_then(|v| v.as_str())
            .is_some_and(|t| t == "primary label")
    }));
    assert!(labels.iter().any(|l| {
        l.get("text")
            .and_then(|v| v.as_str())
            .is_some_and(|t| t == "secondary label")
    }));

    let hints = first
        .get("hints")
        .and_then(|v| v.as_array())
        .expect("hints should be an array");
    assert!(hints.iter().any(|h| {
        h.get("text")
            .and_then(|v| v.as_str())
            .is_some_and(|t| t == "example hint")
    }));
}

#[test]
fn json_render_respects_max_errors_filter() {
    let mk = |line| {
        Diagnostic::make_error_dynamic(
            "E300",
            "TYPE UNIFICATION ERROR",
            ErrorType::Compiler,
            "type mismatch",
            None,
            "test.flx",
            Span::new(Position::new(line, 1), Position::new(line, 2)),
        )
    };
    let out = render_diagnostics_json(&[mk(1), mk(2), mk(3)], Some("test.flx"), Some(2), true);
    let parsed: serde_json::Value =
        serde_json::from_str(&out).expect("expected valid JSON diagnostics output");
    let len = parsed
        .as_array()
        .map(|arr| arr.len())
        .expect("expected array diagnostics output");
    assert_eq!(len, 2, "expected max-errors filtering to cap JSON output");
}
