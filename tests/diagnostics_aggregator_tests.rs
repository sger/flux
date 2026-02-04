use flux::frontend::{
    diagnostics::{Diagnostic, DiagnosticsAggregator, RelatedDiagnostic, render_diagnostics_multi},
    position::{Position, Span},
};

fn set_no_color() {
    unsafe {
        std::env::set_var("NO_COLOR", "1");
    }
}

fn span(line: usize, column: usize) -> Span {
    Span::new(Position::new(line, column), Position::new(line, column + 1))
}

#[test]
fn aggregator_sorts_and_groups_by_file_and_severity() {
    set_no_color();

    let diags = vec![
        Diagnostic::warning("WARN")
            .with_message("warn")
            .with_file("a.flx")
            .with_span(span(1, 0)),
        Diagnostic::error("ERR")
            .with_message("err")
            .with_file("a.flx")
            .with_span(span(1, 0)),
        Diagnostic::error("ERRB")
            .with_message("errb")
            .with_file("b.flx")
            .with_span(span(1, 0)),
    ];

    let output = render_diagnostics_multi(&diags, Some(50));

    let a_idx = output.find("--> a.flx").expect("missing a.flx header");
    let b_idx = output.find("--> b.flx").expect("missing b.flx header");
    assert!(a_idx < b_idx);

    let err_idx = output.find("-- Error: ERR").expect("missing error");
    let warn_idx = output.find("-- Warning: WARN").expect("missing warning");
    assert!(err_idx < warn_idx);
}

#[test]
fn aggregator_prints_summary_counts() {
    set_no_color();

    let diags = vec![
        Diagnostic::error("ERR")
            .with_file("a.flx")
            .with_span(span(1, 0)),
        Diagnostic::warning("WARN")
            .with_file("a.flx")
            .with_span(span(2, 0)),
    ];

    let output = render_diagnostics_multi(&diags, Some(50));
    assert!(output.contains("Found 1 error and 1 warning."));
}

#[test]
fn aggregator_enforces_max_errors() {
    set_no_color();

    let diags = vec![
        Diagnostic::error("E1")
            .with_file("a.flx")
            .with_span(span(1, 0)),
        Diagnostic::error("E2")
            .with_file("a.flx")
            .with_span(span(2, 0)),
        Diagnostic::error("E3")
            .with_file("a.flx")
            .with_span(span(3, 0)),
        Diagnostic::warning("W1")
            .with_file("a.flx")
            .with_span(span(4, 0)),
    ];

    let output = render_diagnostics_multi(&diags, Some(1));
    assert_eq!(output.matches("-- Error:").count(), 1);
    assert!(output.contains("-- Warning: W1"));
    assert!(output.contains("... and 2 more errors not shown (use --max-errors to increase)."));
}

#[test]
fn aggregator_deduplicates_identical_diagnostics() {
    set_no_color();

    let base = Diagnostic::error("DUP")
        .with_code("E123")
        .with_message("same message")
        .with_file("a.flx")
        .with_span(span(1, 0));
    let dup = base.clone();
    let near = Diagnostic::error("DUP")
        .with_code("E123")
        .with_message("different message")
        .with_file("a.flx")
        .with_span(span(1, 0));

    let output = render_diagnostics_multi(&[base, dup, near], Some(50));
    assert_eq!(output.matches("-- Error: DUP").count(), 2);
}

#[test]
fn aggregator_renders_related_diagnostics_in_order() {
    set_no_color();

    let primary = Diagnostic::error("PRIMARY")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_related(RelatedDiagnostic::note("first note"))
        .with_related(RelatedDiagnostic::help("second help"));

    let output = render_diagnostics_multi(&[primary], Some(50));

    let primary_idx = output.find("-- Error: PRIMARY").expect("missing primary");
    let note_idx = output.find("note: first note").expect("missing note");
    let help_idx = output.find("help: second help").expect("missing help");
    assert!(primary_idx < note_idx);
    assert!(note_idx < help_idx);
}

#[test]
fn aggregator_dedupes_related_sets_only_when_matching() {
    set_no_color();

    let with_related = Diagnostic::error("DUPREL")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_related(RelatedDiagnostic::note("note A"));
    let with_related_dup = Diagnostic::error("DUPREL")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_related(RelatedDiagnostic::note("note A"));
    let with_related_diff = Diagnostic::error("DUPREL")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_related(RelatedDiagnostic::note("note B"));

    let output = render_diagnostics_multi(
        &[with_related, with_related_dup, with_related_diff],
        Some(50),
    );
    assert_eq!(output.matches("note:").count(), 2);
}

#[test]
fn aggregator_keeps_diagnostics_with_different_hints() {
    set_no_color();

    let base = Diagnostic::error("HINTDEDUP")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0));
    let with_hint = Diagnostic::error("HINTDEDUP")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_hint_text("extra context");

    let output = render_diagnostics_multi(&[base, with_hint], Some(50));
    assert_eq!(output.matches("-- Error: HINTDEDUP").count(), 2);
    assert!(output.contains("extra context"));
}

#[test]
fn aggregator_dedupes_identical_with_hints() {
    set_no_color();

    let with_hint = Diagnostic::error("HINTDEDUP2")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_hint_text("same context");
    let with_hint_dup = Diagnostic::error("HINTDEDUP2")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_hint_text("same context");

    let output = render_diagnostics_multi(&[with_hint, with_hint_dup], Some(50));
    assert_eq!(output.matches("-- Error: HINTDEDUP2").count(), 1);
    assert_eq!(output.matches("same context").count(), 1);
}

#[test]
fn aggregator_keeps_diagnostics_with_different_labels() {
    set_no_color();

    let source = "let x = 1;\n";
    let with_label_a = Diagnostic::error("LABELDEDUP")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_primary_label(span(1, 0), "label A");
    let with_label_b = Diagnostic::error("LABELDEDUP")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_primary_label(span(1, 0), "label B");

    let output = DiagnosticsAggregator::new(&[with_label_a, with_label_b])
        .with_source("a.flx", source)
        .render();
    assert_eq!(output.matches("-- Error: LABELDEDUP").count(), 2);
    assert!(output.contains("label A"));
    assert!(output.contains("label B"));
}
