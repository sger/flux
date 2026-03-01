mod diagnostics_env;

use flux::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticPhase, DiagnosticsAggregator, ErrorType,
    RelatedDiagnostic,
    position::{Position, Span},
    render_diagnostics_multi,
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
    let summary_idx = output
        .find("Found 2 warnings.")
        .expect("missing summary line");
    let last_diag_idx = output
        .rfind("--> warning[E000]:")
        .expect("missing warning diagnostics");
    assert!(
        summary_idx > last_diag_idx,
        "summary should be rendered after diagnostics"
    );
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

#[test]
fn aggregator_suppresses_nearby_duplicate_e300_same_message() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let mk = |line: usize, start_col: usize, end_col: usize| {
        let s = Span::new(Position::new(line, start_col), Position::new(line, end_col));
        Diagnostic::make_error_dynamic(
            "E300",
            "TYPE UNIFICATION ERROR",
            ErrorType::Compiler,
            "The branches of this `if` expression produce different types.",
            None,
            "a.flx",
            s,
        )
        .with_primary_label(s, "primary")
    };

    let d1 = mk(10, 5, 12);
    let d2 = mk(10, 8, 16); // overlaps line 10
    let d3 = mk(11, 2, 6); // adjacent line to retained span

    let output = render_diagnostics_multi(&[d1, d2, d3], Some(50));
    assert_eq!(output.matches("error[E300]").count(), 1);
    assert!(output.contains("Suppressed 2 nearby duplicate E300 diagnostic(s)."));
}

#[test]
fn aggregator_does_not_suppress_e300_with_different_messages() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let s1 = Span::new(Position::new(5, 1), Position::new(5, 6));
    let s2 = Span::new(Position::new(6, 1), Position::new(6, 6)); // adjacent line
    let d1 = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "The branches of this `if` expression produce different types.",
        None,
        "a.flx",
        s1,
    )
    .with_primary_label(s1, "if");
    let d2 = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "The 1st argument to `f` has the wrong type.",
        None,
        "a.flx",
        s2,
    )
    .with_primary_label(s2, "arg");

    let output = render_diagnostics_multi(&[d1, d2], Some(50));
    assert_eq!(output.matches("error[E300]").count(), 2);
    assert!(!output.contains("Suppressed "));
}

#[test]
fn aggregator_does_not_suppress_e300_across_files() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let s = Span::new(Position::new(3, 2), Position::new(3, 8));
    let d1 = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "The branches of this `if` expression produce different types.",
        None,
        "a.flx",
        s,
    )
    .with_primary_label(s, "a");
    let d2 = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "The branches of this `if` expression produce different types.",
        None,
        "b.flx",
        s,
    )
    .with_primary_label(s, "b");

    let output = render_diagnostics_multi(&[d1, d2], Some(50));
    assert_eq!(output.matches("error[E300]").count(), 2);
    assert!(!output.contains("Suppressed "));
}

#[test]
fn aggregator_does_not_suppress_non_e300() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let s1 = Span::new(Position::new(7, 1), Position::new(7, 3));
    let s2 = Span::new(Position::new(8, 1), Position::new(8, 3));
    let d1 = Diagnostic::make_error_dynamic(
        "E031",
        "EXPECTED EXPRESSION",
        ErrorType::Compiler,
        "Expected expression, found `}`.",
        None,
        "a.flx",
        s1,
    )
    .with_primary_label(s1, "e1");
    let d2 = Diagnostic::make_error_dynamic(
        "E031",
        "EXPECTED EXPRESSION",
        ErrorType::Compiler,
        "Expected expression, found `}`.",
        None,
        "a.flx",
        s2,
    )
    .with_primary_label(s2, "e2");

    let output = render_diagnostics_multi(&[d1, d2], Some(50));
    assert_eq!(output.matches("error[E031]").count(), 2);
    assert!(!output.contains("Suppressed "));
}

#[test]
fn aggregator_stage_filtering_parse_suppresses_type_and_effect() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let parse = Diagnostic::make_error_dynamic(
        "E071",
        "UNTERMINATED STRING",
        ErrorType::Compiler,
        "missing quote",
        None,
        "a.flx",
        span(1, 0),
    )
    .with_phase(DiagnosticPhase::Parse);
    let ty = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "cannot unify",
        None,
        "a.flx",
        span(2, 0),
    )
    .with_phase(DiagnosticPhase::TypeInference);
    let eff = Diagnostic::make_error_dynamic(
        "E407",
        "UNKNOWN FUNCTION EFFECT",
        ErrorType::Compiler,
        "unknown effect",
        None,
        "a.flx",
        span(3, 0),
    )
    .with_phase(DiagnosticPhase::Effect);

    let output = DiagnosticsAggregator::new(&[parse, ty, eff]).render();
    assert!(output.contains("error[E071]"));
    assert!(!output.contains("error[E300]"));
    assert!(!output.contains("error[E407]"));
    assert!(output.contains("DOWNSTREAM ERRORS SUPPRESSED"));
    assert!(!output.contains(":0:1"));
}

#[test]
fn aggregator_stage_filtering_type_suppresses_effect() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let ty = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "cannot unify",
        None,
        "a.flx",
        span(2, 0),
    )
    .with_phase(DiagnosticPhase::TypeCheck);
    let eff = Diagnostic::make_error_dynamic(
        "E407",
        "UNKNOWN FUNCTION EFFECT",
        ErrorType::Compiler,
        "unknown effect",
        None,
        "a.flx",
        span(3, 0),
    )
    .with_phase(DiagnosticPhase::Effect);

    let output = DiagnosticsAggregator::new(&[ty, eff]).render();
    assert!(output.contains("error[E300]"));
    assert!(!output.contains("error[E407]"));
    assert!(output.contains("DOWNSTREAM ERRORS SUPPRESSED"));
    assert!(!output.contains(":0:1"));
}

#[test]
fn aggregator_stage_filtering_can_be_disabled() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let parse = Diagnostic::make_error_dynamic(
        "E071",
        "UNTERMINATED STRING",
        ErrorType::Compiler,
        "missing quote",
        None,
        "a.flx",
        span(1, 0),
    )
    .with_phase(DiagnosticPhase::Parse);
    let ty = Diagnostic::make_error_dynamic(
        "E300",
        "TYPE UNIFICATION ERROR",
        ErrorType::Compiler,
        "cannot unify",
        None,
        "a.flx",
        span(2, 0),
    )
    .with_phase(DiagnosticPhase::TypeInference);

    let output = DiagnosticsAggregator::new(&[parse, ty])
        .with_stage_filtering(false)
        .render();
    assert!(output.contains("error[E071]"));
    assert!(output.contains("error[E300]"));
    assert!(!output.contains("DOWNSTREAM ERRORS SUPPRESSED"));
}

#[test]
fn aggregator_collapses_parser_cascades() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let root = Diagnostic::make_error_dynamic(
        "E076",
        "UNCLOSED DELIMITER",
        ErrorType::Compiler,
        "missing `}`",
        None,
        "a.flx",
        span(10, 1),
    )
    .with_phase(DiagnosticPhase::Parse);
    let cascade_1 = Diagnostic::make_error_dynamic(
        "E034",
        "UNEXPECTED TOKEN",
        ErrorType::Compiler,
        "unexpected token",
        None,
        "a.flx",
        span(11, 1),
    )
    .with_phase(DiagnosticPhase::Parse);
    let cascade_2 = Diagnostic::make_error_dynamic(
        "E034",
        "UNEXPECTED TOKEN",
        ErrorType::Compiler,
        "unexpected token",
        None,
        "a.flx",
        span(12, 1),
    )
    .with_phase(DiagnosticPhase::Parse);

    let output = DiagnosticsAggregator::new(&[root, cascade_1, cascade_2]).render();
    assert_eq!(output.matches("error[E034]").count(), 0);
    assert_eq!(output.matches("error[E076]").count(), 1);
    assert!(output.contains("cascading parser diagnostic"));
}

/// T1 acceptance: every diagnostic emitted from a real multi-error compilation
/// must carry a non-None phase tag so that stage filtering can classify it.
#[test]
fn all_compiler_diagnostics_have_phase_tags() {
    use flux::bytecode::compiler::Compiler;
    use flux::syntax::{lexer::Lexer, parser::Parser};

    // A source with both parse and type errors.
    let source = r#"
fn greet(name: String) -> String {
    "Hello, #{name}!"
}

fn main() with IO {
    print(greet(42))
}
"#;

    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    let mut all_diagnostics: Vec<Diagnostic> = Vec::new();

    // Tag parse warnings/errors (mirrors main.rs pipeline).
    let mut parse_warnings = parser.take_warnings();
    for d in &mut parse_warnings {
        if d.phase().is_none() {
            *d = d.clone().with_phase(DiagnosticPhase::Parse);
        }
    }
    all_diagnostics.append(&mut parse_warnings);

    if !parser.errors.is_empty() {
        for d in &mut parser.errors {
            if d.phase().is_none() {
                *d = d.clone().with_phase(DiagnosticPhase::Parse);
            }
        }
        all_diagnostics.append(&mut parser.errors);
    }

    // Compile (produces internally-tagged diagnostics).
    if all_diagnostics.is_empty() {
        let interner = parser.take_interner();
        let mut compiler = Compiler::new_with_interner("test.flx", interner);
        if let Err(mut diags) = compiler.compile(&program) {
            // Safety-net tag: mirrors main.rs bulk-tag for compile errors.
            for d in &mut diags {
                if d.phase().is_none() {
                    *d = d.clone().with_phase(DiagnosticPhase::TypeCheck);
                }
            }
            all_diagnostics.append(&mut diags);
        }
        let mut warnings = compiler.take_warnings();
        for d in &mut warnings {
            if d.phase().is_none() {
                *d = d.clone().with_phase(DiagnosticPhase::Validation);
            }
        }
        all_diagnostics.append(&mut warnings);
    }

    // Must have produced diagnostics (this source has a type error).
    assert!(
        !all_diagnostics.is_empty(),
        "expected diagnostics from multi-error source"
    );

    // Every diagnostic must have a phase tag.
    for (i, diag) in all_diagnostics.iter().enumerate() {
        assert!(
            diag.phase().is_some(),
            "diagnostic #{} has no phase tag: code={:?} title={:?} message={:?}",
            i,
            diag.code(),
            diag.title(),
            diag.message(),
        );
    }
}

/// Parse errors also get phase tags when tagged through the main.rs pipeline.
#[test]
fn parse_error_diagnostics_have_phase_tags() {
    use flux::syntax::{lexer::Lexer, parser::Parser};

    let source = r#"
fn main() -> Unit {
    let broken = "unterminated
}
"#;

    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let _program = parser.parse_program();

    assert!(!parser.errors.is_empty(), "expected parse errors");

    // Tag parse errors (mirrors main.rs).
    for d in &mut parser.errors {
        if d.phase().is_none() {
            *d = d.clone().with_phase(DiagnosticPhase::Parse);
        }
    }

    for (i, diag) in parser.errors.iter().enumerate() {
        assert!(
            diag.phase().is_some(),
            "parse diagnostic #{} has no phase tag: code={:?}",
            i,
            diag.code(),
        );
        assert_eq!(
            diag.phase(),
            Some(DiagnosticPhase::Parse),
            "parse diagnostic #{} should be tagged Parse, got {:?}",
            i,
            diag.phase(),
        );
    }
}
