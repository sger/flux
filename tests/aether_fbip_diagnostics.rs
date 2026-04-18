use flux::compiler::Compiler;
use flux::diagnostics::Diagnostic;
use flux::syntax::{lexer::Lexer, module_graph::ModuleGraph, parser::Parser};
use std::path::Path;

fn compile_ok_with_warnings(input: &str) -> Vec<Diagnostic> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<aether-fbip>", interner);
    compiler.compile(&program).expect("expected compile ok");
    compiler.take_warnings()
}

fn compile_fixture_warnings(rel: &str) -> Vec<Diagnostic> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = workspace_root.join(rel);
    let source = std::fs::read_to_string(&fixture).expect("fixture should exist");
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );

    let mut roots = Vec::new();
    if let Some(parent) = fixture.parent() {
        roots.push(parent.to_path_buf());
    }
    let src_root = workspace_root.join("src");
    if src_root.exists() {
        roots.push(src_root);
    }

    let graph =
        ModuleGraph::build_with_entry_and_roots(&fixture, &program, parser.take_interner(), &roots);
    assert!(
        graph.diagnostics.is_empty(),
        "module diagnostics: {:?}",
        graph.diagnostics
    );

    let mut compiler = Compiler::new_with_interner(rel, graph.interner);
    for node in graph.graph.topo_order() {
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        compiler.set_current_module_kind(node.kind);
        compiler
            .compile(&node.program)
            .expect("expected compile ok");
    }
    compiler.take_warnings()
}

fn compile_err_diagnostics(input: &str) -> Vec<Diagnostic> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<aether-fbip>", interner);
    compiler
        .compile(&program)
        .expect_err("expected compile error")
}

#[test]
fn fip_warning_reports_fresh_allocation_cause() {
    let src = std::fs::read_to_string("examples/aether/fbip_fail_nonfip_call.flx")
        .expect("fixture should exist")
        .replace("@fbip fn bounded(f, x) {", "@fip fn bounded(f, x) {");
    let warnings = compile_ok_with_warnings(&src);
    assert!(warnings.iter().any(|d| {
        d.message()
            .is_some_and(|m| m.contains("indirect or opaque callee `f`"))
    }));
}

#[test]
fn fbip_failure_is_hard_error() {
    let src = std::fs::read_to_string("examples/aether/fbip_fail_nonfip_call.flx")
        .expect("fixture should exist");
    let diagnostics = compile_err_diagnostics(&src);
    assert!(diagnostics.iter().any(|d| {
        d.message()
            .is_some_and(|m| m.contains("indirect or opaque callee `f`"))
    }));
}

#[test]
fn vacuous_annotation_is_advisory_warning() {
    let src =
        std::fs::read_to_string("examples/aether/fbip_vacuous.flx").expect("fixture should exist");
    let warnings = compile_ok_with_warnings(&src);
    assert!(warnings.iter().any(|d| {
        d.title() == "FBIP Annotation Has No Effect"
            && d.message()
                .is_some_and(|m| m.contains("no heap constructor sites"))
    }));
}

#[test]
fn verify_aether_my_map_reports_higher_order_blocker_not_self_recursion_noise() {
    let src =
        std::fs::read_to_string("examples/aether/verify_aether.flx").expect("fixture should exist");
    let warnings = compile_ok_with_warnings(&src);
    let my_map_warning = warnings
        .iter()
        .find(|d| d.message().is_some_and(|m| m.contains("@fip on `my_map`")))
        .expect("my_map warning should exist");
    let message = my_map_warning.message().expect("warning message");
    assert!(message.contains("indirect or opaque callee `f`"));
    assert!(
        !message.contains("calls known function `my_map` whose FBIP behavior is not yet provable"),
        "self-recursive higher-order fixtures should not report recursive non-provable noise when stronger blockers already exist"
    );
}

#[test]
fn fbip_failure_cases_cover_current_warning_categories() {
    let warnings = compile_fixture_warnings("examples/aether/fbip_failure_cases.flx");

    assert!(
        warnings.iter().any(|d| {
            d.message()
                .is_some_and(|m| m.contains("indirect or opaque callee `f`"))
        }),
        "expected indirect higher-order warning"
    );
    assert!(
        warnings.iter().any(|d| {
            d.message().is_some_and(|m| {
                m.contains("Imported.imported_inc")
                    || m.contains("imported_inc")
                    || m.contains("indirect or opaque callee")
            })
        }),
        "expected imported/name-only warning"
    );
    assert!(
        warnings.iter().any(|d| {
            d.message()
                .is_some_and(|m| m.contains("fresh heap allocation remains"))
        }),
        "expected fresh-allocation warning"
    );
}
