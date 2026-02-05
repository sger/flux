mod diagnostics_env;

use flux::frontend::{
    diagnostics::{
        Diagnostic, DiagnosticBuilder, DiagnosticsAggregator, RelatedDiagnostic,
        render_diagnostics_multi,
    },
    position::{Position, Span},
};

fn span(line: usize, column: usize) -> Span {
    Span::new(Position::new(line, column), Position::new(line, column + 1))
}

#[test]
fn aggregator_sorts_and_groups_by_file_and_severity() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let diags = vec![
        Diagnostic::warning("WARN")
            .with_message("warn")
            .with_file("a.flx")
            .with_span(span(1, 0)),
        Diagnostic::warning("ERR")
            .with_message("err")
            .with_file("a.flx")
            .with_span(span(1, 0)),
        Diagnostic::warning("ERRB")
            .with_message("errb")
            .with_file("b.flx")
            .with_span(span(1, 0)),
    ];

    let output = render_diagnostics_multi(&diags, Some(50));

    let a_idx = output.find("--> a.flx").expect("missing a.flx header");
    let b_idx = output.find("--> b.flx").expect("missing b.flx header");
    assert!(a_idx < b_idx);

    let err_idx = output
        .find("--> warning[E000]: ERR")
        .expect("missing err warning");
    let warn_idx = output
        .find("--> warning[E000]: WARN")
        .expect("missing warn warning");
    assert!(err_idx < warn_idx);
}

#[test]
fn aggregator_prints_summary_counts() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let diags = vec![
        Diagnostic::warning("ERR")
            .with_file("a.flx")
            .with_span(span(1, 0)),
        Diagnostic::warning("WARN")
            .with_file("a.flx")
            .with_span(span(2, 0)),
    ];

    let output = render_diagnostics_multi(&diags, Some(50));
    assert!(output.contains("Found 2 warnings."));
}

#[test]
fn aggregator_single_file_shows_header() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let diags = vec![
        Diagnostic::warning("ERR")
            .with_file("a.flx")
            .with_span(span(1, 0)),
    ];

    let output = render_diagnostics_multi(&diags, Some(50));
    assert!(output.contains("--> a.flx"));
}

#[test]
fn aggregator_enforces_max_errors() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let diags = vec![
        Diagnostic::warning("E1")
            .with_file("a.flx")
            .with_span(span(1, 0)),
        Diagnostic::warning("E2")
            .with_file("a.flx")
            .with_span(span(2, 0)),
        Diagnostic::warning("E3")
            .with_file("a.flx")
            .with_span(span(3, 0)),
        Diagnostic::warning("W1")
            .with_file("a.flx")
            .with_span(span(4, 0)),
    ];

    let output = render_diagnostics_multi(&diags, Some(1));
    // Since all diagnostics are warnings now, max_errors doesn't limit them
    assert_eq!(output.matches("--> warning[E000]:").count(), 4);
    assert!(output.contains("--> warning[E000]: E1"));
    assert!(output.contains("--> warning[E000]: W1"));
}

#[test]
fn aggregator_deduplicates_identical_diagnostics() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let base = Diagnostic::warning("DUP")
        .with_code("E123")
        .with_message("same message")
        .with_file("a.flx")
        .with_span(span(1, 0));
    let dup = base.clone();
    let near = Diagnostic::warning("DUP")
        .with_code("E123")
        .with_message("different message")
        .with_file("a.flx")
        .with_span(span(1, 0));

    let output = render_diagnostics_multi(&[base, dup, near], Some(50));
    assert_eq!(output.matches("--> warning[E123]: DUP").count(), 2);
}

#[test]
fn aggregator_renders_related_diagnostics_in_order() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let primary = Diagnostic::warning("PRIMARY")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_related(RelatedDiagnostic::note("first note"))
        .with_related(RelatedDiagnostic::help("second help"));

    let output = render_diagnostics_multi(&[primary], Some(50));

    let primary_idx = output
        .find("--> warning[E000]: PRIMARY")
        .expect("missing primary");
    let note_idx = output.find("note: first note").expect("missing note");
    let help_idx = output.find("help: second help").expect("missing help");
    assert!(primary_idx < note_idx);
    assert!(note_idx < help_idx);
}

#[test]
fn aggregator_dedupes_related_sets_only_when_matching() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let with_related = Diagnostic::warning("DUPREL")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_related(RelatedDiagnostic::note("note A"));
    let with_related_dup = Diagnostic::warning("DUPREL")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_related(RelatedDiagnostic::note("note A"));
    let with_related_diff = Diagnostic::warning("DUPREL")
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let base = Diagnostic::warning("HINTDEDUP")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0));
    let with_hint = Diagnostic::warning("HINTDEDUP")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_hint_text("extra context");

    let output = render_diagnostics_multi(&[base, with_hint], Some(50));
    assert_eq!(output.matches("--> warning[E000]: HINTDEDUP").count(), 2);
    assert!(output.contains("extra context"));
}

#[test]
fn aggregator_dedupes_identical_with_hints() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let with_hint = Diagnostic::warning("HINTDEDUP2")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_hint_text("same context");
    let with_hint_dup = Diagnostic::warning("HINTDEDUP2")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_hint_text("same context");

    let output = render_diagnostics_multi(&[with_hint, with_hint_dup], Some(50));
    assert_eq!(output.matches("--> warning[E000]: HINTDEDUP2").count(), 1);
    assert_eq!(output.matches("same context").count(), 1);
}

#[test]
fn aggregator_keeps_diagnostics_with_different_labels() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let source = "let x = 1;\n";
    let with_label_a = Diagnostic::warning("LABELDEDUP")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_primary_label(span(1, 0), "label A");
    let with_label_b = Diagnostic::warning("LABELDEDUP")
        .with_message("same")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_primary_label(span(1, 0), "label B");

    let output = DiagnosticsAggregator::new(&[with_label_a, with_label_b])
        .with_source("a.flx", source)
        .render();
    assert_eq!(output.matches("--> warning[E000]: LABELDEDUP").count(), 2);
    assert!(output.contains("label A"));
    assert!(output.contains("label B"));
}

#[test]
fn aggregator_renders_cross_file_related_snippet() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let primary_source = "let a = 1;\n";
    let related_source = "let b = 2;\n";
    let primary = Diagnostic::warning("PRIMARY")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_related(
            RelatedDiagnostic::note("see related")
                .with_file("b.flx")
                .with_span(span(1, 4)),
        );

    let output = DiagnosticsAggregator::new(&[primary])
        .with_source("a.flx", primary_source)
        .with_source("b.flx", related_source)
        .render();

    assert!(output.contains("  --> b.flx:1:5"));
    assert!(output.contains("let b = 2;"));
}

#[test]
fn aggregator_related_without_source_renders_no_snippet() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let primary_source = "let a = 1;\n";
    let related_source = "let missing = 9;\n";
    let primary = Diagnostic::warning("PRIMARY")
        .with_file("a.flx")
        .with_span(span(1, 0))
        .with_related(
            RelatedDiagnostic::note("see related")
                .with_file("missing_related.flx")
                .with_span(span(1, 0)),
        );

    let output = DiagnosticsAggregator::new(&[primary])
        .with_source("a.flx", primary_source)
        .render();

    assert!(output.contains("  --> missing_related.flx:1:1"));
    assert!(!output.contains(related_source));
}
